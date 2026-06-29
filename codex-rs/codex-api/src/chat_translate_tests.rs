use super::*;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use serde_json::json;

fn user_msg(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn assistant_msg(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn developer_msg(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "developer".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn function_call(name: &str, args: &str, call_id: &str) -> ResponseItem {
    ResponseItem::FunctionCall {
        id: None,
        name: name.to_string(),
        namespace: None,
        arguments: args.to_string(),
        call_id: call_id.to_string(),
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn instructions_become_leading_system_message() {
    let messages = responses_input_to_chat_messages(&[], "you are helpful");
    assert_eq!(messages.len(), 1);
    match &messages[0] {
        ChatMessage::Text { role, content } => {
            assert_eq!(role, "system");
            assert_eq!(content, "you are helpful");
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn no_instructions_means_no_system_message() {
    let messages = responses_input_to_chat_messages(&[user_msg("hi")], "");
    assert!(
        messages
            .iter()
            .all(|m| !matches!(m, ChatMessage::Text { role, .. } if role == "system"))
    );
}

#[test]
fn developer_role_aliases_to_system() {
    let messages = responses_input_to_chat_messages(&[developer_msg("rules")], "");
    assert_eq!(messages.len(), 1);
    match &messages[0] {
        ChatMessage::Text { role, .. } => assert_eq!(role, "system"),
        other => panic!("expected system, got {other:?}"),
    }
}

#[test]
fn consecutive_same_role_text_messages_are_merged() {
    let messages = responses_input_to_chat_messages(&[user_msg("hello"), user_msg("world")], "");
    assert_eq!(messages.len(), 1);
    match &messages[0] {
        ChatMessage::Text { role, content } => {
            assert_eq!(role, "user");
            assert_eq!(content, "hello\nworld");
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn assistant_then_user_keeps_separate_messages() {
    let messages = responses_input_to_chat_messages(&[assistant_msg("hi"), user_msg("bye")], "");
    assert_eq!(messages.len(), 2);
}

#[test]
fn parallel_function_calls_group_into_one_assistant_message() {
    let messages = responses_input_to_chat_messages(
        &[
            function_call("read", "{}", "call_1"),
            function_call("write", "{}", "call_2"),
        ],
        "",
    );
    assert_eq!(messages.len(), 1);
    match &messages[0] {
        ChatMessage::AssistantWithToolCalls {
            tool_calls,
            content,
            role,
        } => {
            assert_eq!(role, "assistant");
            assert!(content.is_none());
            assert_eq!(tool_calls.len(), 2);
            assert_eq!(tool_calls[0].function.name, "read");
            assert_eq!(tool_calls[0].id, "call_1");
            assert_eq!(tool_calls[1].function.name, "write");
        }
        other => panic!("expected AssistantWithToolCalls, got {other:?}"),
    }
}

#[test]
fn function_call_output_becomes_tool_role_message() {
    let messages = responses_input_to_chat_messages(
        &[ResponseItem::FunctionCallOutput {
            id: None,
            call_id: "call_1".to_string(),
            output: FunctionCallOutputPayload::from_text("result data".to_string()),
            internal_chat_message_metadata_passthrough: None,
        }],
        "",
    );
    assert_eq!(messages.len(), 1);
    match &messages[0] {
        ChatMessage::ToolResult {
            role,
            tool_call_id,
            content,
        } => {
            assert_eq!(role, "tool");
            assert_eq!(tool_call_id, "call_1");
            assert_eq!(content, "result data");
        }
        other => panic!("expected ToolResult, got {other:?}"),
    }
}

#[test]
fn reasoning_items_are_dropped() {
    let messages = responses_input_to_chat_messages(
        &[
            user_msg("hi"),
            ResponseItem::Reasoning {
                id: None,
                summary: vec![],
                content: None,
                encrypted_content: None,
                internal_chat_message_metadata_passthrough: None,
            },
        ],
        "",
    );
    assert_eq!(messages.len(), 1);
}

#[test]
fn full_conversation_round_trip() {
    // A representative agent loop fragment: assistant calls a tool, tool
    // returns a result, then assistant replies with text.
    let input = vec![
        user_msg("what's the weather?"),
        function_call("get_weather", r#"{"city":"SF"}"#, "call_1"),
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: "call_1".to_string(),
            output: FunctionCallOutputPayload::from_text("sunny".to_string()),
            internal_chat_message_metadata_passthrough: None,
        },
        assistant_msg("It's sunny in SF."),
    ];
    let messages = responses_input_to_chat_messages(&input, "be concise");

    // system, user, assistant(tool_calls), tool, assistant(text)
    assert_eq!(messages.len(), 5);
    assert!(matches!(messages[0], ChatMessage::Text { ref role, .. } if role == "system"));
    assert!(matches!(messages[3], ChatMessage::ToolResult { .. }));
}

#[test]
fn convert_function_tool_preserves_schema() {
    let tools = vec![json!({
        "type": "function",
        "name": "get_weather",
        "description": "Get weather for a city",
        "strict": true,
        "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}
    })];
    let chat_tools = responses_tools_to_chat_tools(&tools);
    assert_eq!(chat_tools.len(), 1);
    let value = serde_json::to_value(&chat_tools[0]).unwrap();
    assert_eq!(value["type"], "function");
    assert_eq!(value["function"]["name"], "get_weather");
    assert_eq!(value["function"]["strict"], true);
    assert_eq!(value["function"]["parameters"]["type"], "object");
}

#[test]
fn convert_drops_non_function_tools() {
    // web_search / image_generation / tool_search have no chat equivalent.
    let tools = vec![
        json!({"type": "web_search"}),
        json!({"type": "function", "name": "ok", "description": "", "strict": false, "parameters": {}}),
        json!({"type": "image_generation", "output_format": "png"}),
    ];
    let chat_tools = responses_tools_to_chat_tools(&tools);
    assert_eq!(chat_tools.len(), 1);
    let value = serde_json::to_value(&chat_tools[0]).unwrap();
    assert_eq!(value["function"]["name"], "ok");
    // strict=false should be omitted (skip_serializing_if).
    assert!(value["function"].get("strict").is_none());
}

#[test]
fn strict_false_is_omitted_from_serialized_tool() {
    let value = json!({
        "type": "function",
        "name": "n",
        "description": "",
        "strict": false,
        "parameters": {}
    });
    let chat_tools = responses_tools_to_chat_tools(&[value]);
    let serialized = serde_json::to_value(&chat_tools[0]).unwrap();
    assert!(serialized["function"].get("strict").is_none());
}
