//! Relay images out of tool results into a following user message.
//!
//! The OpenAI-compatible chat wire format (OpenAI, OpenRouter, Ollama, …)
//! cannot carry images in `role: "tool"` messages, so rig's converters drop
//! `ToolResultContent::Image` parts (OpenRouter emits a placeholder string;
//! plain OpenAI errors the turn). Anthropic and Gemini accept them natively,
//! but a single uniform path keeps behavior identical across providers — so
//! [`ImageRelayModel`] wraps every agent's completion model and rewrites each
//! final request: image parts are stripped from tool results and re-sent as a
//! plain user message right after them, the one multimodal surface every
//! OpenAI-compatible API shares.
//!
//! This runs at the wire boundary (the assembled [`CompletionRequest`]),
//! because the tool-result message becomes the *prompt* of the continuation
//! turn inside rig's run loop — a `RequestPatch::history` hook cannot reach
//! it. Rewriting here changes only what is sent; the persisted transcript is
//! untouched.
//!
//! The long-term fix is rig 0.41+ (typed `ToolOutput`, native MCP image
//! content, ordered mixed user/tool-result blocks); this shim is only needed
//! on rig 0.40.

use rig::OneOrMany;
use rig::completion::message::{DocumentSourceKind, Image, UserContent};
use rig::completion::request::{
    CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
};
use rig::message::{Message, ToolResult, ToolResultContent};
use rig::streaming::StreamingCompletionResponse;

/// Decorator around any [`CompletionModel`] that hoists images out of tool
/// results into a following user message before the request goes on the wire.
#[derive(Clone)]
pub struct ImageRelayModel<M>(M);

impl<M> ImageRelayModel<M> {
    pub fn new(model: M) -> Self {
        Self(model)
    }
}

impl<M: CompletionModel> CompletionModel for ImageRelayModel<M> {
    type Response = M::Response;
    type StreamingResponse = M::StreamingResponse;
    type Client = M::Client;

    fn make(client: &Self::Client, model: impl Into<String>) -> Self {
        Self(M::make(client, model))
    }

    async fn completion(
        &self,
        mut request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        relay_request(&mut request);
        self.0.completion(request).await
    }

    async fn stream(
        &self,
        mut request: CompletionRequest,
    ) -> Result<StreamingCompletionResponse<Self::StreamingResponse>, CompletionError> {
        relay_request(&mut request);
        self.0.stream(request).await
    }

    fn composes_native_output_with_tools(&self) -> bool {
        self.0.composes_native_output_with_tools()
    }
}

fn relay_request(request: &mut CompletionRequest) {
    let history: Vec<Message> = request.chat_history.clone().into_iter().collect();
    if let Some(rewritten) = relay_images(&history) {
        request.chat_history = OneOrMany::many(rewritten).expect("rewritten history is non-empty");
    }
}

/// Move images found in tool results into a following user message.
///
/// Returns `None` when the history contains no tool-result images, so
/// uninteresting requests pass through untouched.
fn relay_images(history: &[Message]) -> Option<Vec<Message>> {
    let mut changed = false;
    let mut out = Vec::with_capacity(history.len() + 1);
    for msg in history {
        let Message::User { content } = msg else {
            out.push(msg.clone());
            continue;
        };
        let has_image = content.iter().any(|c| {
            matches!(c, UserContent::ToolResult(tr)
                if tr.content.iter().any(|p| matches!(p, ToolResultContent::Image(_))))
        });
        if !has_image {
            out.push(msg.clone());
            continue;
        }
        changed = true;
        let mut kept: Vec<UserContent> = Vec::new();
        let mut images: Vec<UserContent> = Vec::new();
        for part in content.clone() {
            match part {
                UserContent::ToolResult(tr) => kept.extend(split_tool_result(tr, &mut images)),
                other => kept.push(other),
            }
        }
        out.push(Message::User {
            content: OneOrMany::many(kept).expect("tool-result message keeps at least one part"),
        });
        if !images.is_empty() {
            out.push(Message::User {
                content: OneOrMany::many(images).expect("checked non-empty above"),
            });
        }
    }
    changed.then_some(out)
}

/// Strip image parts out of one tool result, queueing them as user image
/// content, and note the move in the remaining text.
fn split_tool_result(tr: ToolResult, images: &mut Vec<UserContent>) -> Vec<UserContent> {
    let image_count = tr
        .content
        .iter()
        .filter(|p| matches!(p, ToolResultContent::Image(_)))
        .count();
    let mut texts: Vec<String> = Vec::new();
    for part in tr.content {
        match part {
            ToolResultContent::Text(t) => texts.push(t.text),
            ToolResultContent::Image(img) => images.push(image_part(img)),
        }
    }
    let mut text = texts.join("\n");
    if image_count > 0 {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!(
            "[This tool result also contained {image_count} image(s); they follow in the next message.]"
        ));
    }
    vec![UserContent::ToolResult(ToolResult {
        id: tr.id,
        call_id: tr.call_id,
        content: OneOrMany::one(text.into()),
    })]
}

fn image_part(img: Image) -> UserContent {
    match img.data {
        DocumentSourceKind::Base64(data) => {
            UserContent::image_base64(data, img.media_type, img.detail)
        }
        DocumentSourceKind::Url(url) => UserContent::image_url(url, img.media_type, img.detail),
        // Raw bytes / provider FileId are not produced by the MCP
        // envelope; encode Raw with `base64` if a native tool ever returns it.
        _ => UserContent::text("[unsupported image source dropped]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig::completion::message::ImageMediaType;

    fn tool_result_msg(id: &str, parts: Vec<ToolResultContent>) -> Message {
        Message::User {
            content: OneOrMany::one(UserContent::ToolResult(ToolResult {
                id: id.to_string(),
                call_id: None,
                content: OneOrMany::many(parts).expect("test parts non-empty"),
            })),
        }
    }

    fn png_image(data: &str) -> ToolResultContent {
        ToolResultContent::Image(Image {
            data: DocumentSourceKind::Base64(data.to_string()),
            media_type: Some(ImageMediaType::PNG),
            detail: None,
            additional_params: None,
        })
    }

    #[test]
    fn history_without_tool_images_is_left_untouched() {
        let history = vec![
            Message::user("hello"),
            tool_result_msg("call-1", vec![ToolResultContent::text("plain output")]),
        ];
        assert!(relay_images(&history).is_none());
    }

    #[test]
    fn images_move_into_following_user_message() {
        let history = vec![tool_result_msg(
            "call-1",
            vec![ToolResultContent::text("screenshot ok"), png_image("aGk=")],
        )];

        let out = relay_images(&history).expect("images should trigger a rewrite");
        assert_eq!(out.len(), 2);

        let Message::User { content } = &out[0] else {
            panic!("expected user message");
        };
        assert_eq!(content.len(), 1);
        let UserContent::ToolResult(tr) = content.first() else {
            panic!("expected tool result");
        };
        assert_eq!(tr.id, "call-1");
        let text = match tr.content.first() {
            ToolResultContent::Text(t) => t.text.clone(),
            other => panic!("expected text, got {other:?}"),
        };
        assert!(text.starts_with("screenshot ok"));
        assert!(text.ends_with(
            "[This tool result also contained 1 image(s); they follow in the next message.]"
        ));

        let Message::User { content } = &out[1] else {
            panic!("expected image user message");
        };
        assert_eq!(content.len(), 1);
        let UserContent::Image(img) = content.first() else {
            panic!("expected image part");
        };
        assert_eq!(img.data, DocumentSourceKind::Base64("aGk=".to_string()));
        assert_eq!(img.media_type, Some(ImageMediaType::PNG));
    }

    #[test]
    fn non_tool_result_parts_keep_their_positions() {
        let history = vec![Message::User {
            content: OneOrMany::many(vec![
                UserContent::text("before"),
                UserContent::ToolResult(ToolResult {
                    id: "call-1".into(),
                    call_id: None,
                    content: OneOrMany::one(png_image("aGk=")),
                }),
                UserContent::text("after"),
            ])
            .expect("test parts non-empty"),
        }];

        let out = relay_images(&history).expect("images should trigger a rewrite");
        assert_eq!(out.len(), 2);

        let Message::User { content } = &out[0] else {
            panic!("expected user message");
        };
        assert_eq!(content.len(), 3);
        let parts: Vec<String> = content
            .iter()
            .map(|c| match c {
                UserContent::Text(t) => t.text.clone(),
                UserContent::ToolResult(tr) => match tr.content.first() {
                    ToolResultContent::Text(t) => t.text.clone(),
                    other => panic!("expected note text, got {other:?}"),
                },
                other => panic!("unexpected part {other:?}"),
            })
            .collect();
        assert_eq!(parts[0], "before");
        assert!(parts[1].contains("[This tool result also contained 1 image(s)"));
        assert_eq!(parts[2], "after");
    }

    #[test]
    fn url_images_pass_through_as_urls() {
        let history = vec![tool_result_msg(
            "call-1",
            vec![ToolResultContent::Image(Image {
                data: DocumentSourceKind::Url("https://example.com/i.png".to_string()),
                media_type: Some(ImageMediaType::PNG),
                detail: None,
                additional_params: None,
            })],
        )];

        let out = relay_images(&history).expect("images should trigger a rewrite");
        let Message::User { content } = &out[1] else {
            panic!("expected image user message");
        };
        let UserContent::Image(img) = content.first() else {
            panic!("expected image part");
        };
        assert_eq!(
            img.data,
            DocumentSourceKind::Url("https://example.com/i.png".to_string())
        );
    }

    #[test]
    fn multiple_images_across_messages_all_land_in_user_messages() {
        let history = vec![
            tool_result_msg("call-1", vec![png_image("one"), png_image("two")]),
            tool_result_msg("call-2", vec![png_image("three")]),
        ];

        let out = relay_images(&history).expect("images should trigger a rewrite");
        assert_eq!(out.len(), 4);
        let count = |msg: &Message| match msg {
            Message::User { content } => content
                .iter()
                .filter(|c| matches!(c, UserContent::Image(_)))
                .count(),
            _ => 0,
        };
        assert_eq!(count(&out[1]), 2);
        assert_eq!(count(&out[3]), 1);
    }
}
