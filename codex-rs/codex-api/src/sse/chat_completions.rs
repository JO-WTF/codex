//! SSE event processing for the Chat Completions API stream.
//!
//! The Chat Completions API returns events with different event types than the Responses API.
//! The key mappings are:
//! - `chat.completion.chunk` → text delta
//! - `chat.completion.token_usage` → token counts
//! - `chat.completion.error` → error handling
//! - `chat.completion.done` → stream completion
//! - `chat.completion.function_call` → function call input
//!
//! Each event has its own structure. We map them into the internal `ResponseEvent` enum.

use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::rate_limits::parse_all_rate_limits;
use crate::telemetry::SseTelemetry;
use codex_client::ByteStream;
use codex_client::StreamResponse;
use codex_protocol::protocol::TokenUsage;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

const X_CODEX_TURN_STATE_HEADER: &str = "x-codex-turn-state";
const REQUEST_ID_HEADER: &str = "x-request-id";

pub fn spawn_chat_completions_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    turn_state: Option<Arc<OnceLock<String>>>,
) -> ResponseStream {
    let rate_limit_snapshots = parse_all_rate_limits(&stream_response.headers);
    let upstream_request_id = stream_response
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    if let Some(turn_state) = turn_state.as_ref()
        && let Some(header_value) = stream_response
            .headers
            .get(X_CODEX_TURN_STATE_HEADER)
            .and_then(|value| value.to_str().ok())
    {
        let _ = turn_state.set(header_value.to_string());
    }
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        for snapshot in rate_limit_snapshots {
            let _ = tx_event.send(Ok(ResponseEvent::RateLimits(snapshot))).await;
        }
        process_chat_completions_sse(stream_response.bytes, tx_event, idle_timeout, telemetry)
            .await;
    });

    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ChatError {
    r#type: Option<String>,
    code: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ChatCompletionChunk {
    id: String,
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
    created: Option<i64>,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    delta: Option<ChatDelta>,
    finish_reason: Option<String>,
    index: i64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ChatDelta {
    content: Option<String>,
    role: Option<String>,
    tool_calls: Option<Vec<ChatToolCall>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ChatToolCall {
    function: Option<ChatFunctionCall>,
    id: String,
    r#type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ChatFunctionCall {
    arguments: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    completion_tokens: i64,
    prompt_tokens: i64,
    total_tokens: i64,
}

async fn process_chat_completions_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) {
    let mut stream = stream.eventsource();

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("SSE Error: {e:#}");
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "stream closed before chat completion".into(),
                    )))
                    .await;
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream("idle timeout waiting for SSE".into())))
                    .await;
                return;
            }
        };

        trace!("SSE event: {}", &sse.data);

        // Chat Completions specific: we receive `[DONE]` as the last event
        if sse.data.trim() == "[DONE]" {
            // Stream completed, this is the end marker
            return;
        }

        let chunk: ChatCompletionChunk = match serde_json::from_str(&sse.data) {
            Ok(chunk) => chunk,
            Err(e) => {
                debug!(
                    "Failed to parse ChatCompletionChunk: {e}, data: {}",
                    &sse.data
                );
                continue;
            }
        };

        // Process each choice's delta
        for choice in chunk.choices {
            if let Some(delta) = choice.delta {
                // Handle text content delta
                if let Some(content) = delta.content {
                    let _ = tx_event
                        .send(Ok(ResponseEvent::OutputTextDelta(content)))
                        .await;
                }
                // Handle tool calls in delta
                if let Some(tool_calls) = delta.tool_calls {
                    for tool_call in tool_calls {
                        if let Some(function) = tool_call.function
                            && let Some(arguments) = function.arguments
                        {
                            let _ = tx_event
                                .send(Ok(ResponseEvent::ToolCallInputDelta {
                                    item_id: tool_call.id.clone(),
                                    call_id: Some(tool_call.id.clone()),
                                    delta: arguments,
                                }))
                                .await;
                        }
                    }
                }
            }

            // Handle finish reason - send completed when finish_reason is "stop"
            if let Some(finish_reason) = &choice.finish_reason
                && (finish_reason == "stop" || finish_reason == "length")
            {
                let id = chunk.id.clone();
                let usage = chunk.usage.as_ref().map(|u| TokenUsage {
                    input_tokens: u.prompt_tokens,
                    cached_input_tokens: 0,
                    output_tokens: u.completion_tokens,
                    reasoning_output_tokens: 0,
                    total_tokens: u.total_tokens,
                });
                // Only send completed event for the first choice to avoid duplicates
                if choice.index == 0 {
                    let _ = tx_event
                        .send(Ok(ResponseEvent::Completed {
                            response_id: id,
                            token_usage: usage,
                            end_turn: Some(true),
                        }))
                        .await;
                }
            }
        }
    }
}
