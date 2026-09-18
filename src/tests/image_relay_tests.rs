//! End-to-end check that MCP tool results carrying images reach the LLM as
//! real multimodal parts on OpenAI-compatible providers.
//!
//! The MCP tool emits rig's multimodal envelope (`src/extras/mcp/tool.rs`),
//! rig parses it into `ToolResultContent::Image` parts, and the
//! `ImageRelayModel` (`src/agent/image_relay.rs`) moves those parts into a
//! following user message — because the OpenAI-compatible `role: "tool"`
//! message cannot carry images (OpenRouter drops them with a placeholder,
//! plain OpenAI errors the turn). The scripted fake model captures the exact
//! `chat_history` each turn receives, so this test asserts on what would go
//! over the wire.

use rig::OneOrMany;
use rig::agent::AgentBuilder;
use rig::completion::message::{DocumentSourceKind, ImageMediaType, UserContent};
use rig::message::{Message, ToolResultContent};
use rig::tool::{Tool, ToolError};

use crate::agent::image_relay::ImageRelayModel;
use crate::tests::fake_model::{MockCompletionModel, MockStreamEvent};

const ENVELOPE: &str = r#"{"response":"screenshot captured","parts":[{"type":"image","data":"aGVsbG8=","mimeType":"image/png"}]}"#;

struct ShotTool;

impl Tool for ShotTool {
    const NAME: &'static str = "shot";
    type Error = ToolError;
    type Args = ();
    type Output = String;

    fn description(&self) -> String {
        "Takes a screenshot".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _args: Self::Args) -> Result<String, Self::Error> {
        Ok(ENVELOPE.to_string())
    }
}

fn shot_tool_model() -> MockCompletionModel {
    MockCompletionModel::from_stream_turns(vec![
        vec![
            MockStreamEvent::tool_call("call-1", "shot", serde_json::json!(null)),
            MockStreamEvent::final_response_with_default_usage(),
        ],
        vec![
            MockStreamEvent::text("done".to_string()),
            MockStreamEvent::final_response_with_default_usage(),
        ],
    ])
}

#[tokio::test]
async fn tool_result_image_is_relayed_as_user_message() {
    let model = shot_tool_model();
    let agent = AgentBuilder::new(ImageRelayModel::new(model.clone()))
        .preamble("test")
        .default_max_turns(4)
        .tool(ShotTool)
        .build();

    use futures::StreamExt;
    use rig::streaming::StreamingChat;
    let mut stream = agent
        .stream_chat("take a screenshot", Vec::<Message>::new())
        .await;
    while let Some(item) = stream.next().await {
        item.expect("stream should complete without error");
    }

    let history = |turn: usize| -> Vec<Message> {
        let requests = model.requests();
        requests[turn].chat_history.clone().into_iter().collect()
    };

    // Turn 0 must not have been patched (no tool results yet).
    let initial = history(0);
    assert!(
        !initial.iter().any(|m| matches!(m,
            Message::User { content }
            if content.iter().any(|c| matches!(c, UserContent::ToolResult(_)))
        )),
        "turn 0 history should carry no tool results: {initial:?}"
    );

    // Turn 1's wire request: system, prompt, assistant tool call, the tool
    // result with the image stripped and noted, then a user message carrying
    // the image part.
    let history = history(1);
    assert_eq!(
        history.len(),
        5,
        "expected system + prompt + assistant call + tool result + relayed images"
    );

    let Message::User { content } = &history[3] else {
        panic!("expected user tool-result message");
    };
    let UserContent::ToolResult(tr) = content.first() else {
        panic!("expected tool result part");
    };
    let ToolResultContent::Text(t) = tr.content.first() else {
        panic!("expected the tool result to be text-only after the relay");
    };
    assert!(
        t.text.contains("screenshot captured"),
        "response text should survive: {}",
        t.text
    );
    assert!(
        t.text.contains(
            "[This tool result also contained 1 image(s); they follow in the next message.]"
        ),
        "note about the moved image missing: {}",
        t.text
    );

    let Message::User { content } = &history[4] else {
        panic!("expected relayed image user message");
    };
    assert_eq!(content.len(), 1);
    let UserContent::Image(img) = content.first() else {
        panic!("expected an image part, got {:?}", content.first());
    };
    assert_eq!(img.data, DocumentSourceKind::Base64("aGVsbG8=".to_string()));
    assert_eq!(img.media_type, Some(ImageMediaType::PNG));
}

/// Without the relay, rig's OpenAI-compatible converters would drop the image
/// (OpenRouter placeholder) or error (OpenAI); this pins the envelope's
/// parsing into real `ToolResultContent::Image` parts inside the run state.
#[test]
fn envelope_parses_into_text_and_image_parts() {
    let parsed: OneOrMany<ToolResultContent> =
        rig::completion::message::ToolResultContent::from_tool_output(ENVELOPE);
    assert_eq!(parsed.len(), 2);
    let mut parts = parsed.iter();
    assert!(matches!(parts.next(), Some(ToolResultContent::Text(_))));
    assert!(matches!(parts.next(), Some(ToolResultContent::Image(_))));
}
