//! Translation layer between Codex's Responses-shaped data model and the
//! OpenAI-compatible Chat Completions wire format.
//!
//! Third-party providers (DeepSeek, OpenRouter, vLLM, Ollama's OpenAI shim,
//! LM Studio, etc.) overwhelmingly speak the Chat Completions API
//! (`POST /v1/chat/completions`), not the Responses API. This module bridges
//! the gap so a `wire_api = "chat"` provider can reuse all of Codex's
//! Responses-shaped session state (`ResponseItem`, tool specs, reasoning) by
//! translating at the request boundary.
//!
//! Translation is intentionally **lossy but safe**: Responses-only concepts
//! (stateful `store`/`previous_response_id`, encrypted reasoning, server-side
//! `include`, OpenAI-hosted tools like `web_search`/`image_generation`) have
//! no Chat Completions equivalent and are dropped or approximated. The core
//! agent loop runs identically regardless of wire protocol because both paths
//! produce the same [`crate::common::ResponseEvent`] stream.

use codex_protocol::models::AgentMessageInputContent;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// The reasoning effort hint expressed in the way most chat-compatible
/// providers understand it (OpenAI o-series and DeepSeek-R1 style).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatReasoningEffort {
    Low,
    Medium,
    High,
}

/// A single message in the Chat Completions `messages` array.
///
/// The shapes here deliberately follow the OpenAI Chat Completions schema so
/// that it serializes directly into a valid request body.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(untagged)]
pub enum ChatMessage {
    /// Plain text/content message: `{ "role", "content" }`.
    Text { role: String, content: String },
    /// Assistant message that carries tool calls.
    AssistantWithToolCalls {
        role: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        tool_calls: Vec<ChatToolCall>,
    },
    /// The result of a tool call: `{ "role": "tool", "tool_call_id", "content" }`.
    ToolResult {
        role: String,
        tool_call_id: String,
        content: String,
    },
}

/// A tool call attached to an assistant message, in Chat Completions shape:
/// `{ "id", "type": "function", "function": { "name", "arguments" } }`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChatToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub function: ChatToolCallFunction,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChatToolCallFunction {
    pub name: String,
    /// Raw JSON arguments string, exactly as in the Responses `FunctionCall`.
    pub arguments: String,
}

/// A tool definition in Chat Completions shape:
/// `{ "type": "function", "function": { "name", "description", "strict", "parameters" } }`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChatTool {
    #[serde(rename = "type")]
    pub r#type: String,
    pub function: ChatToolFunction,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChatToolFunction {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Whether the provider should strictly validate tool arguments against
    /// the schema. Many chat-compatible providers do not support this flag;
    /// it is only emitted when `true`.
    #[serde(skip_serializing_if = "skip_false")]
    pub strict: bool,
    pub parameters: Value,
}

fn skip_false(value: &bool) -> bool {
    !value
}

/// Converts a Responses-style tool spec list (`create_tools_json_for_responses_api`
/// output) into the Chat Completions `tools` array shape.
///
/// Only function tools are representable on the chat wire; other tool kinds
/// (`web_search`, `image_generation`, `tool_search`, OpenAI-hosted namespace
/// tools) are Responses-only and silently dropped because a third-party
/// provider cannot honor them.
pub fn responses_tools_to_chat_tools(tools: &[Value]) -> Vec<ChatTool> {
    let mut out = Vec::with_capacity(tools.len());
    for tool in tools {
        if let Some(converted) = convert_tool_value(tool) {
            out.push(converted);
        }
    }
    out
}

fn convert_tool_value(tool: &Value) -> Option<ChatTool> {
    // The Responses tool spec is a tagged union `{ "type": "<variant>", ... }`.
    let kind = tool.get("type")?.as_str()?;
    if kind != "function" {
        // Non-function tools (web_search, image_generation, tool_search,
        // namespace, custom) have no Chat Completions equivalent for a generic
        // third-party provider. Skip them.
        return None;
    }

    let name = tool.get("name").and_then(Value::as_str)?.to_string();
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let strict = tool.get("strict").and_then(Value::as_bool).unwrap_or(false);
    let parameters = tool
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

    Some(ChatTool {
        r#type: "function".to_string(),
        function: ChatToolFunction {
            name,
            description,
            strict,
            parameters,
        },
    })
}

/// Flatten a Responses `input` array into a Chat Completions `messages` array.
///
/// `instructions` (the top-level `instructions` field of a Responses request)
/// becomes a leading `system` message when non-empty, matching how most chat
/// providers expect a system/developer preamble.
///
/// Mapping rules:
/// - `ResponseItem::Message{role}` → `{role, content}`, where `developer`
///   aliases to `system` (most chat providers only know `system`). Content
///   items are concatenated into a single string (text only; images dropped
///   for now as multimodal support varies by provider).
/// - `ResponseItem::FunctionCall` → an assistant message
///   carrying `tool_calls`.
/// - `ResponseItem::FunctionCallOutput` → a `{role:"tool", tool_call_id,
///   content}` message.
/// - `ResponseItem::Reasoning` → dropped (no chat equivalent; the model cannot
///   act on encrypted/server-side reasoning from a third-party provider).
/// - `ResponseItem::LocalShellCall` and other Responses-native tool calls are
///   approximated as assistant tool calls when they carry a usable call_id.
/// - `ResponseItem::AdditionalTools` carries inline tool defs; ignored here
///   (tools are passed separately via [`responses_tools_to_chat_tools`]).
///
/// Consecutive tool calls share one assistant message (Chat Completions allows
/// multiple `tool_calls` per assistant turn), and each tool result is its own
/// `tool` message keyed by `tool_call_id`.
pub fn responses_input_to_chat_messages(
    input: &[ResponseItem],
    instructions: &str,
) -> Vec<ChatMessage> {
    let mut messages = Vec::with_capacity(input.len() + 1);

    if !instructions.is_empty() {
        messages.push(ChatMessage::Text {
            role: "system".to_string(),
            content: instructions.to_string(),
        });
    }

    for item in input {
        match item {
            ResponseItem::Message { role, content, .. } => {
                let normalized_role = normalize_role(role);
                let text = content
                    .iter()
                    .filter_map(content_item_to_text)
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.trim().is_empty() {
                    continue;
                }
                push_or_merge_text(&mut messages, normalized_role, text);
            }
            ResponseItem::AgentMessage { content, .. } => {
                let text = content
                    .iter()
                    .filter_map(|c| match c {
                        AgentMessageInputContent::InputText { text } => Some(text.as_str()),
                        AgentMessageInputContent::EncryptedContent { .. } => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.trim().is_empty() {
                    continue;
                }
                push_or_merge_text(&mut messages, "assistant".to_string(), text);
            }
            ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            } => {
                let tool_call = ChatToolCall {
                    id: call_id.clone(),
                    r#type: "function".to_string(),
                    function: ChatToolCallFunction {
                        name: name.clone(),
                        arguments: arguments.clone(),
                    },
                };
                push_assistant_tool_call(&mut messages, tool_call);
            }
            ResponseItem::FunctionCallOutput {
                call_id, output, ..
            } => {
                let content = output.body.to_text().unwrap_or_default();
                messages.push(ChatMessage::ToolResult {
                    role: "tool".to_string(),
                    tool_call_id: call_id.clone(),
                    content,
                });
            }
            // Responses-only concepts with no chat equivalent.
            ResponseItem::Reasoning { .. }
            | ResponseItem::AdditionalTools { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::CustomToolCallOutput { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::CompactionTrigger {}
            | ResponseItem::ContextCompaction { .. }
            | ResponseItem::Other => {}
            // Best-effort: model the shell call as an assistant tool call when
            // it carries a call_id the model can reference. Shell calls without
            // a call_id (legacy form) are dropped to avoid confusing the model.
            ResponseItem::LocalShellCall { call_id, .. } => {
                if let Some(call_id) = call_id.as_ref() {
                    let tool_call = ChatToolCall {
                        id: call_id.clone(),
                        r#type: "function".to_string(),
                        function: ChatToolCallFunction {
                            name: "local_shell".to_string(),
                            arguments: String::from("{}"),
                        },
                    };
                    push_assistant_tool_call(&mut messages, tool_call);
                }
            }
        }
    }

    messages
}

/// Map Responses roles onto the set most chat providers accept.
fn normalize_role(role: &str) -> String {
    match role {
        // `developer` is Responses-specific; chat providers typically only
        // recognize `system`. Aliasing keeps the preamble visible.
        "developer" | "system" => "system".to_string(),
        "user" => "user".to_string(),
        "assistant" => "assistant".to_string(),
        // Pass through anything else (some providers accept `tool`, etc.).
        other => other.to_string(),
    }
}

fn content_item_to_text(item: &ContentItem) -> Option<String> {
    match item {
        ContentItem::InputText { text } | ContentItem::OutputText { text } => {
            if text.is_empty() {
                None
            } else {
                Some(text.clone())
            }
        }
        // Multimodal content is intentionally not translated here: support for
        // image inputs varies widely across chat-compatible providers. A
        // future enhancement can emit the `{type:"image_url"}` part array.
        ContentItem::InputImage { .. } => None,
    }
}

/// Append a plain text message, merging with a preceding same-role text
/// message to keep the conversation compact.
fn push_or_merge_text(messages: &mut Vec<ChatMessage>, role: String, text: String) {
    if let Some(ChatMessage::Text {
        role: last_role,
        content,
    }) = messages.last_mut()
    {
        if *last_role == role {
            content.push('\n');
            content.push_str(&text);
            return;
        }
    }
    messages.push(ChatMessage::Text {
        role,
        content: text,
    });
}

/// Append a tool call to the trailing assistant message if it is an
/// `AssistantWithToolCalls`, otherwise start a new one. Chat Completions
/// groups a turn's parallel tool calls under a single assistant message.
fn push_assistant_tool_call(messages: &mut Vec<ChatMessage>, tool_call: ChatToolCall) {
    if let Some(ChatMessage::AssistantWithToolCalls { tool_calls, .. }) = messages.last_mut() {
        tool_calls.push(tool_call);
        return;
    }
    messages.push(ChatMessage::AssistantWithToolCalls {
        role: "assistant".to_string(),
        content: None,
        tool_calls: vec![tool_call],
    });
}

#[cfg(test)]
#[path = "chat_translate_tests.rs"]
mod tests;
