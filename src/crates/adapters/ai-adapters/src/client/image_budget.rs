//! Model-specific image input budgets for an outgoing request copy.

#[cfg(feature = "vision-input")]
pub(super) mod context;
#[cfg(all(test, feature = "vision-input"))]
mod context_tests;

use crate::client::format::ApiFormat;
use crate::types::Message;
use anyhow::Result;
use openbitfun_core_types::errors::{AiProviderError, ErrorCategory};
use serde_json::{json, Value};

/// Message payload overrides are merged after the message projection. Reject
/// them when a budget is configured rather than accepting an uncounted payload.
pub(super) fn validate_extra_body(
    max_images: Option<usize>,
    extra_body: Option<&Value>,
) -> Result<()> {
    if max_images.is_some()
        && extra_body.and_then(Value::as_object).is_some_and(|fields| {
            ["messages", "input", "contents"]
                .iter()
                .any(|key| fields.contains_key(*key))
        })
    {
        return Err(AiProviderError::classified(
            "A model image limit cannot be combined with custom messages, input or contents. Remove the message payload override and use ordinary model messages.".into(),
            ErrorCategory::InvalidRequest,
        )
        .into());
    }
    Ok(())
}

/// Send pixels for new inputs and retain references to images in history.
/// Keep the newest pending images within the selected model's count limit.
/// Within one message, retain the first images in their original order. Tool
/// identities, text, reasoning and replay state are otherwise left intact.
pub(super) fn project(
    mut messages: Vec<Message>,
    max_images: Option<usize>,
    turn_start: Option<usize>,
    format: ApiFormat,
    retain_latest_user_image: bool,
) -> Result<Vec<Message>> {
    project_history(&mut messages, turn_start, format, retain_latest_user_image);
    let Some(mut remaining) = max_images else {
        return Ok(messages);
    };
    if remaining == 0 {
        return Err(AiProviderError::classified(
            "max_images_per_request must be greater than zero".into(),
            ErrorCategory::InvalidRequest,
        )
        .into());
    }

    for message in messages.iter_mut().rev() {
        match message.role.as_str() {
            "tool" => {
                let Some(images) = message.tool_image_attachments.as_mut() else {
                    continue;
                };
                let total = images.len();
                let keep = total.min(remaining);
                remaining -= keep;
                let omitted = total - keep;
                if omitted > 0 {
                    images.truncate(keep);
                    if images.is_empty() {
                        message.tool_image_attachments = None;
                    }
                    let text = message.content.get_or_insert_with(String::new);
                    let retained = if keep > 0 {
                        format!(" Only the first {keep} of {total} images from this tool result are included.")
                    } else {
                        String::new()
                    };
                    text.push_str(&format!(
                        "\n\n[{omitted} tool image(s) omitted to fit the model's per-request image limit.{retained} Read images in smaller batches if their pixels are needed.]"
                    ));
                }
            }
            "user" => project_user_content(&mut message.content, &mut remaining, format),
            _ => {}
        }
    }
    Ok(messages)
}

/// A gateway can reject a batch even when all its images fit the context window.
pub(super) fn smaller_tool_batch(messages: &[Message]) -> Option<Vec<Message>> {
    let count = messages
        .iter()
        .filter(|message| message.role == "tool")
        .map(|message| message.tool_image_attachments.as_ref().map_or(0, Vec::len))
        .sum::<usize>();
    if count < 2 {
        return None;
    }
    let mut keep_one = true;
    let mut reduced = Vec::with_capacity(messages.len());
    for message in messages.iter().rev() {
        let attachments = message
            .tool_image_attachments
            .as_deref()
            .unwrap_or_default();
        if message.role != "tool" || attachments.is_empty() {
            reduced.push(message.clone());
            continue;
        }
        let keep = usize::from(std::mem::take(&mut keep_one));
        let mut adjusted = Message {
            content: message.content.clone(),
            role: message.role.clone(),
            reasoning_content: message.reasoning_content.clone(),
            thinking_signature: message.thinking_signature.clone(),
            tool_calls: message.tool_calls.clone(),
            tool_call_id: message.tool_call_id.clone(),
            name: message.name.clone(),
            is_error: message.is_error,
            model_response_replay: message.model_response_replay.clone(),
            tool_image_attachments: (keep > 0).then(|| attachments[..keep].to_vec()),
        };
        if attachments.len() > keep {
            let text = adjusted.content.get_or_insert_with(String::new);
            text.push_str("\n[Some tool images were omitted to fit the request body. Read the images in separate calls if needed.]");
        }
        reduced.push(adjusted);
    }
    reduced.reverse();
    Some(reduced)
}

/// A response confirms receipt of all preceding inputs. Consecutive user
/// messages or parallel tool results after it may still await their first send.
fn project_history(
    messages: &mut [Message],
    turn_start: Option<usize>,
    format: ApiFormat,
    retain_latest_user_image: bool,
) {
    let turn_start = turn_start.unwrap_or(0).min(messages.len());
    let pending_start = messages[turn_start..]
        .iter()
        .rposition(|message| message.role == "assistant")
        .map_or(turn_start, |index| turn_start + index + 1);
    let retained = retain_latest_user_image
        .then(|| {
            messages.iter().rposition(|message| {
                message.role == "user"
                    && user_parts(&message.content, format)
                        .is_some_and(|parts| parts.iter().any(is_image_part))
            })
        })
        .flatten();
    for (index, message) in messages[..pending_start].iter_mut().enumerate() {
        match message.role.as_str() {
            "tool" => {
                if let Some(images) = message
                    .tool_image_attachments
                    .take()
                    .filter(|images| !images.is_empty())
                {
                    let text = message.content.get_or_insert_with(String::new);
                    text.push_str(&format!(
                        "\n[{} earlier tool image(s). Read the source again if pixels are needed.]",
                        images.len()
                    ));
                }
            }
            "user" if Some(index) != retained => {
                let Some(mut parts) = user_parts(&message.content, format) else {
                    continue;
                };
                let mut changed = false;
                for part in parts.iter_mut().filter(|part| is_image_part(part)) {
                    let url = &part["image_url"];
                    let reference = url["url"]
                        .as_str()
                        .or_else(|| url.as_str())
                        .or_else(|| part["source"]["url"].as_str())
                        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
                        .or_else(|| part["file_id"].as_str())
                        .or_else(|| part["source"]["file_id"].as_str());
                    let text = match reference {
                        Some(reference) => format!("[Earlier image reference: {reference}]"),
                        None => "[Earlier image. Reattach or read it again if pixels are needed.]"
                            .into(),
                    };
                    *part = json!({"type":"text", "text":text});
                    changed = true;
                }
                if changed {
                    message.content = Some(Value::Array(parts).to_string());
                }
            }
            _ => {}
        }
    }
}

// OpenAI treats an unrecognized array as literal text. Gemini recognizes
// individual image parts alongside unknown siblings; Anthropic forwards the array.
fn user_parts(content: &Option<String>, format: ApiFormat) -> Option<Vec<Value>> {
    let content = content.as_deref()?.trim_start();
    if !content.starts_with('[') {
        return None;
    }
    let parts: Vec<Value> = serde_json::from_str(content).ok()?;
    if matches!(format, ApiFormat::OpenAIChat | ApiFormat::OpenAIResponses)
        && !parts.iter().all(is_content_part)
    {
        return None;
    }
    if matches!(format, ApiFormat::Gemini | ApiFormat::GeminiCodeAssist)
        && !crate::providers::GeminiMessageConverter::is_user_content_parts(&parts)
    {
        return None;
    }
    Some(parts)
}

fn project_user_content(content: &mut Option<String>, remaining: &mut usize, format: ApiFormat) {
    let Some(mut parts) = user_parts(content, format) else {
        return;
    };
    let image_count = parts.iter().filter(|part| is_image_part(part)).count();
    let keep = image_count.min(*remaining);
    *remaining -= keep;
    let omitted = image_count - keep;
    if omitted == 0 {
        return;
    }
    let mut seen = 0usize;
    parts.retain(|part| {
        if is_image_part(part) {
            seen += 1;
            seen <= keep
        } else {
            true
        }
    });
    let retained = if keep > 0 {
        format!(" Only the first {keep} of {image_count} images from this message are included.")
    } else {
        String::new()
    };
    parts.push(json!({
        "type": "text",
        "text": format!("[{omitted} image(s) omitted to fit the model's per-request image limit.{retained} Reattach or read images in smaller batches if their pixels are needed.]"),
    }));
    *content = Some(Value::Array(parts).to_string());
}

fn image_url(value: Option<&Value>) -> bool {
    value
        .and_then(|value| value.as_str().or_else(|| value.get("url")?.as_str()))
        .is_some_and(|url| !url.is_empty())
}

fn inline_media(part: &Value) -> Option<(&str, &str)> {
    let data = part.get("inline_data").or_else(|| part.get("inlineData"))?;
    Some((
        data.get("mime_type")
            .or_else(|| data.get("mimeType"))?
            .as_str()?,
        data.get("data")?.as_str()?,
    ))
}

fn is_image_part(part: &Value) -> bool {
    match part.get("type").and_then(Value::as_str) {
        Some("image_url") => image_url(part.get("image_url")),
        Some("input_image") => {
            image_url(part.get("image_url"))
                || part.get("file_id").and_then(Value::as_str).is_some()
        }
        Some("image") => part.get("source").is_some_and(|source| {
            image_url(source.get("url"))
                || (source["type"] == "file"
                    && source["file_id"].as_str().is_some_and(|id| !id.is_empty()))
                || (source
                    .get("media_type")
                    .and_then(Value::as_str)
                    .is_some_and(|mime| mime.starts_with("image/"))
                    && source.get("data").and_then(Value::as_str).is_some())
        }),
        _ => inline_media(part).is_some_and(|(mime, _)| mime.starts_with("image/")),
    }
}

fn is_content_part(part: &Value) -> bool {
    if is_image_part(part) || inline_media(part).is_some() {
        return true;
    }
    match part.get("type").and_then(Value::as_str) {
        Some("text" | "input_text" | "output_text") | None => {
            part.get("text").and_then(Value::as_str).is_some()
        }
        // Preserve non-image multimodal parts without charging the image budget.
        Some("input_audio" | "audio" | "video" | "file" | "input_file" | "document") => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::anthropic::message_converter::AnthropicMessageConverter;
    use crate::providers::gemini::message_converter::GeminiMessageConverter;
    use crate::providers::openai::message_converter::OpenAIMessageConverter;
    use crate::types::ToolImageAttachment;

    fn project(messages: Vec<Message>, max_images: Option<usize>) -> Result<Vec<Message>> {
        super::project(messages, max_images, None, ApiFormat::OpenAIChat, false)
    }

    fn image(id: &str) -> Value {
        json!({"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{id}")}})
    }

    fn user_images(ids: &[&str]) -> Message {
        let mut parts = vec![json!({"type": "text", "text": "Inspect these images."})];
        parts.extend(ids.iter().map(|id| image(id)));
        Message::user(Value::Array(parts).to_string())
    }

    fn tool_images(id: &str, count: usize) -> Message {
        let mut message = Message::user("Image attached".into());
        message.role = "tool".into();
        message.tool_call_id = Some(id.into());
        message.name = Some("view_image".into());
        message.tool_image_attachments = Some(
            (0..count)
                .map(|index| ToolImageAttachment {
                    mime_type: "image/png".into(),
                    data_base64: format!("{id}_{index}"),
                })
                .collect(),
        );
        message
    }

    fn count(messages: &[Message]) -> usize {
        messages
            .iter()
            .map(|message| {
                let tool_count = message.tool_image_attachments.as_ref().map_or(0, Vec::len);
                let user_count = if message.role == "user" {
                    message
                        .content
                        .as_deref()
                        .and_then(|content| serde_json::from_str::<Value>(content).ok())
                        .and_then(|value| value.as_array().cloned())
                        .map_or(0, |parts| {
                            parts.iter().filter(|part| is_image_part(part)).count()
                        })
                } else {
                    0
                };
                tool_count + user_count
            })
            .sum()
    }

    fn wire_count(value: &Value) -> usize {
        match value {
            Value::Array(items) => items.iter().map(wire_count).sum(),
            Value::Object(fields) => {
                usize::from(is_image_part(value)) + fields.values().map(wire_count).sum::<usize>()
            }
            _ => 0,
        }
    }

    fn client_config(format: &str, url: &str, limit: usize) -> crate::types::AIConfig {
        serde_json::from_value(json!({
            "name": "image-budget-test",
            "base_url": url,
            "request_url": url,
            "api_key": "synthetic",
            "model": "test",
            "format": format,
            "context_window": 4096,
            "max_images_per_request": limit,
            "inline_think_in_text": false,
            "skip_ssl_verify": false
        }))
        .unwrap()
    }

    #[test]
    fn mixed_user_and_tool_images_share_the_request_budget() {
        let original = vec![user_images(&["old-a", "old-b"]), tool_images("latest", 2)];
        let original_json = serde_json::to_value(&original).unwrap();
        let projected = project(original.clone(), Some(3)).unwrap();
        assert_eq!(count(&projected), 3);
        let content = projected[0].content.as_deref().unwrap();
        assert!(content.contains("old-a"));
        assert!(!content.contains("old-b"));
        assert!(content.contains("1 image(s) omitted"));
        assert!(content.contains("first 1 of 2 images"));
        assert_eq!(projected[1].tool_call_id.as_deref(), Some("latest"));
        assert_eq!(serde_json::to_value(original).unwrap(), original_json);
    }

    #[test]
    fn four_parallel_tool_results_keep_the_latest_three_without_losing_results() {
        let messages = (0..4)
            .map(|index| tool_images(&format!("call-{index}"), 1))
            .collect();
        let projected = project(messages, Some(3)).unwrap();
        assert_eq!(projected.len(), 4);
        assert_eq!(count(&projected), 3);
        assert!(projected[0].tool_image_attachments.is_none());
        assert!(projected[0]
            .content
            .as_deref()
            .unwrap()
            .contains("Read images in smaller batches"));
        for (index, message) in projected.iter().enumerate() {
            assert_eq!(message.tool_call_id, Some(format!("call-{index}")));
            assert_eq!(message.name.as_deref(), Some("view_image"));
            assert!(message
                .content
                .as_deref()
                .unwrap()
                .starts_with("Image attached"));
        }
    }

    #[test]
    fn one_tool_result_can_be_partially_projected() {
        let mut original = tool_images("batch", 4);
        original.reasoning_content = Some("keep reasoning".into());
        original.thinking_signature = Some("keep signature".into());
        original.is_error = Some(false);
        original.model_response_replay = Some(
            serde_json::from_value(json!({
                "protocol": "test", "items": []
            }))
            .unwrap(),
        );
        let mut expected = serde_json::to_value(&original).unwrap();
        let projected = project(vec![original], Some(3)).unwrap();
        let images = projected[0].tool_image_attachments.as_ref().unwrap();
        assert_eq!(images.len(), 3);
        assert_eq!(images[0].data_base64, "batch_0");
        assert_eq!(images[2].data_base64, "batch_2");
        assert!(projected[0]
            .content
            .as_deref()
            .unwrap()
            .contains("1 tool image(s) omitted"));
        assert!(projected[0]
            .content
            .as_deref()
            .unwrap()
            .contains("first 3 of 4 images"));
        // The content placeholder and truncated attachment vector are the only
        // permitted differences; metadata and result identities remain intact.
        let actual = serde_json::to_value(&projected[0]).unwrap();
        expected["content"] = actual["content"].clone();
        expected["tool_image_attachments"] = actual["tool_image_attachments"].clone();
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_plain_continue_keeps_a_repeatable_text_reference() {
        let mut history = vec![tool_images("batch", 4)];
        assert_eq!(count(&project(history.clone(), Some(5)).unwrap()), 4);
        assert_eq!(count(&project(history.clone(), Some(3)).unwrap()), 3);
        history.push(Message::assistant("I have inspected the images.".into()));
        history.push(Message::user("Continue.".into()));
        let projected = project(history.clone(), Some(3)).unwrap();
        assert_eq!(count(&projected), 0);
        assert_eq!(projected[2].content.as_deref(), Some("Continue."));
        assert_eq!(count(&history), 4);
        let expected = serde_json::to_value(&projected).unwrap();
        assert_eq!(
            serde_json::to_value(project(projected, Some(3)).unwrap()).unwrap(),
            expected
        );
    }

    #[test]
    fn unconfigured_and_within_budget_requests_are_byte_for_byte_unchanged() {
        let mut message = user_images(&["a", "b", "c", "d"]);
        message.content = Some(format!("\n {} \n", message.content.unwrap()));
        message.reasoning_content = Some("Reasoning remains attached.".into());
        message.thinking_signature = Some("signature".into());
        let messages = vec![message, tool_images("tool", 1)];
        let expected = serde_json::to_value(&messages).unwrap();
        assert_eq!(
            serde_json::to_value(project(messages.clone(), None).unwrap()).unwrap(),
            expected
        );
        assert_eq!(
            serde_json::to_value(project(messages, Some(5)).unwrap()).unwrap(),
            expected
        );
    }

    #[test]
    fn zero_limit_is_a_structured_invalid_request() {
        let error = project(vec![Message::user("hello".into())], Some(0)).unwrap_err();
        let error = error.downcast_ref::<AiProviderError>().unwrap();
        assert_eq!(error.category, ErrorCategory::InvalidRequest);
        assert!(error.provider.is_none());
        assert!(error.http_status.is_none());
    }

    #[test]
    fn message_payload_overrides_cannot_bypass_a_configured_image_limit() {
        for key in ["messages", "input", "contents"] {
            let override_body = json!({(key): [image("custom")]});
            let error = validate_extra_body(Some(3), Some(&override_body)).unwrap_err();
            let error = error.downcast_ref::<AiProviderError>().unwrap();
            assert_eq!(error.category, ErrorCategory::InvalidRequest);
            assert!(error.provider.is_none());
            assert!(error.http_status.is_none());
            assert!(error
                .message
                .contains("Remove the message payload override"));
            assert!(validate_extra_body(None, Some(&override_body)).is_ok());
        }
        assert!(validate_extra_body(Some(3), None).is_ok());
        assert!(validate_extra_body(
            Some(3),
            Some(&json!({
                "temperature": 0.2, "thinking": {"type": "enabled"}
            }))
        )
        .is_ok());
    }

    #[tokio::test]
    async fn every_send_entry_point_rejects_a_zero_budget_before_transport() {
        for format in ["openai", "responses", "anthropic", "gemini"] {
            let config = client_config(format, "invalid-image-budget-test", 0);
            let client = crate::AIClient::new(config);
            let messages = vec![Message::user("hello".into())];
            let errors = [
                client
                    .send_message_stream_once(messages.clone(), None, None)
                    .await
                    .err()
                    .unwrap(),
                client
                    .send_message_once_with_trace_and_request_context(
                        messages.clone(),
                        None,
                        None,
                        None,
                    )
                    .await
                    .err()
                    .unwrap(),
            ];
            for error in errors {
                assert_eq!(
                    error.downcast_ref::<AiProviderError>().unwrap().category,
                    ErrorCategory::InvalidRequest
                );
            }
            for key in ["messages", "input", "contents"] {
                let mut bounded = client.clone();
                bounded.config.max_images_per_request = Some(3);
                bounded.config.custom_request_body = Some(json!({(key): []}));
                let error = bounded
                    .send_message_stream_once(messages.clone(), None, None)
                    .await
                    .err()
                    .unwrap();
                assert_eq!(
                    error.downcast_ref::<AiProviderError>().unwrap().category,
                    ErrorCategory::InvalidRequest
                );
            }
        }
    }

    #[tokio::test]
    async fn selected_client_budget_is_applied_to_each_actual_http_send() {
        use axum::{extract::State, routing::post, Json, Router};
        use std::sync::{Arc, Mutex};

        async fn receive(
            State(requests): State<Arc<Mutex<Vec<Value>>>>,
            Json(body): Json<Value>,
        ) -> ([(&'static str, &'static str); 1], String) {
            requests.lock().unwrap().push(body);
            let content_chunk = json!({
                "id": "image-budget-test",
                "object": "chat.completion.chunk",
                "created": 1,
                "model": "test",
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant", "content": "ok"},
                    "finish_reason": null
                }]
            });
            let stop_chunk = json!({
                "id": "image-budget-test",
                "object": "chat.completion.chunk",
                "created": 1,
                "model": "test",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
            });
            (
                [("content-type", "text/event-stream")],
                format!("data: {content_chunk}\n\ndata: {stop_chunk}\n\ndata: [DONE]\n\n"),
            )
        }

        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/chat/completions", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/chat/completions", post(receive))
            .with_state(requests.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let initial = crate::AIClient::new(client_config("openai", &url, 5));
        let mut selected = initial.clone();
        selected.config.max_images_per_request = Some(3);
        let mut history = vec![tool_images("batch", 4)];
        // Use actual one-pixel images so the 4K window exercises the count limit.
        for image in history[0].tool_image_attachments.as_mut().unwrap() {
            image.data_base64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVR4nGNgAAIAAAUAAXpeqz8AAAAASUVORK5CYII=".into();
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            initial
                .send_message_once_with_trace_and_request_context(history.clone(), None, None, None)
                .await
                .unwrap();
            selected
                .send_message_once_with_trace_and_request_context(history.clone(), None, None, None)
                .await
                .unwrap();
            history.push(Message::assistant("I inspected the images.".into()));
            history.push(Message::user("Continue.".into()));
            selected
                .send_message_once_with_trace_and_request_context(history.clone(), None, None, None)
                .await
                .unwrap();
        })
        .await
        .unwrap();
        server.abort();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(wire_count(&requests[0]["messages"]), 4);
        assert_eq!(wire_count(&requests[1]["messages"]), 3);
        assert_eq!(wire_count(&requests[2]["messages"]), 0);
        assert_eq!(count(&history), 4);
    }

    #[test]
    fn historical_image_parts_preserve_text_and_existing_references() {
        let inline = image("unrepeatable-pixels");
        let url = "https://example.com/diagram.png";
        let known = Message::user(
            json!([
                {"type":"text","text":"Compare these images"},
                inline, {"type":"image_url","image_url":{"url":url}},
                {"type":"input_image","file_id":"file-diagram"}
            ])
            .to_string(),
        );
        let literal = Message::user(json!([inline, {"record":"application data"}]).to_string());
        let messages = vec![
            known,
            literal,
            Message::assistant("Done".into()),
            user_images(&["new"]),
            Message::user("Review the details.".into()),
        ];
        let original = serde_json::to_value(&messages).unwrap();
        let projected = super::project(
            messages.clone(),
            None,
            None,
            ApiFormat::OpenAIResponses,
            false,
        )
        .unwrap();
        let old = projected[0].content.as_deref().unwrap();
        assert!(
            old.contains(url)
                && old.contains("file-diagram")
                && old.contains("Compare these images")
        );
        assert!(!old.contains("unrepeatable-pixels"));
        assert_eq!(projected[1].content, messages[1].content);
        assert_eq!(projected[3].content, messages[3].content);
        assert_eq!(serde_json::to_value(messages).unwrap(), original);
    }

    #[test]
    fn tool_json_is_opaque_even_if_it_looks_like_image_content() {
        let mut message = tool_images("opaque", 0);
        message.content = user_images(&["a", "b", "c", "d"]).content;
        let expected = serde_json::to_value(&message).unwrap();
        assert_eq!(
            serde_json::to_value(&project(vec![message], Some(1)).unwrap()[0]).unwrap(),
            expected
        );
    }

    #[test]
    fn ordinary_json_and_non_image_inline_media_do_not_consume_the_budget() {
        let ordinary = Message::user(json!({"images": [image("metadata")]}).to_string());
        let array = Message::user(json!([{"record": image("metadata")}]).to_string());
        let audio = Message::user(
            json!([
                {"inlineData": {"mimeType": "audio/wav", "data": "sound"}},
                {"type": "text", "text": "transcribe"}
            ])
            .to_string(),
        );
        let expected = serde_json::to_value([&ordinary, &array, &audio]).unwrap();
        let projected = project(
            vec![ordinary, array, audio, tool_images("latest", 1)],
            Some(1),
        )
        .unwrap();
        assert_eq!(serde_json::to_value(&projected[..3]).unwrap(), expected);
        assert_eq!(count(&projected), 1);
    }

    #[test]
    fn recognizes_all_image_content_dialects_and_keeps_non_image_parts() {
        let dialects = [
            image("pixels"),
            json!({"type": "input_image", "image_url": "data:image/png;base64,pixels"}),
            json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "pixels"}}),
            json!({"inline_data": {"mime_type": "image/png", "data": "pixels"}}),
            json!({"inlineData": {"mimeType": "image/png", "data": "pixels"}}),
        ];
        for image_part in dialects {
            let text = json!({"type": "text", "text": "Preserve this text."});
            let parts = vec![
                image_part.clone(),
                text.clone(),
                image_part.clone(),
                image_part,
            ];
            let projected =
                project(vec![Message::user(json!(parts).to_string())], Some(2)).unwrap();
            let parts: Vec<Value> =
                serde_json::from_str(projected[0].content.as_deref().unwrap()).unwrap();
            assert_eq!(parts.iter().filter(|part| is_image_part(part)).count(), 2);
            assert_eq!(parts[1], text);
            assert!(parts.last().unwrap()["text"]
                .as_str()
                .unwrap()
                .contains("1 image(s) omitted"));
        }
    }

    #[test]
    fn consumed_images_keep_their_user_and_tool_roles_on_the_wire() {
        let call = crate::ToolCall {
            id: "capture".into(),
            name: "view_image".into(),
            arguments: json!({"path":"screen.png"}),
            raw_arguments: None,
        };
        let history = vec![
            user_images(&["upload"]),
            Message::assistant_with_tools(vec![call]),
            tool_images("capture", 1),
            Message::assistant("Reviewed both images.".into()),
            Message::user("Continue.".into()),
        ];
        for (format, model) in [
            (ApiFormat::OpenAIChat, "test"),
            (ApiFormat::OpenAIResponses, "test"),
            (ApiFormat::Anthropic, "test"),
            (ApiFormat::Gemini, "gemini-3-pro"),
            (ApiFormat::Gemini, "gemini-2.5-pro"),
        ] {
            let projected = super::project(history.clone(), None, None, format, false).unwrap();
            let wire = match format {
                ApiFormat::OpenAIChat => OpenAIMessageConverter::convert_messages(projected),
                ApiFormat::OpenAIResponses => {
                    OpenAIMessageConverter::convert_messages_to_responses_input(projected).1
                }
                ApiFormat::Anthropic => AnthropicMessageConverter::convert_messages(projected).1,
                _ => GeminiMessageConverter::convert_messages(projected, model).1,
            };
            assert_eq!(wire[0]["role"], "user");
            assert!(wire[0].to_string().contains("Inspect these images"));
            let result = &wire[2];
            match format {
                ApiFormat::OpenAIChat => {
                    assert_eq!(result["role"], "tool");
                    assert_eq!(result["tool_call_id"], "capture");
                }
                ApiFormat::OpenAIResponses => {
                    assert_eq!(result["type"], "function_call_output");
                    assert_eq!(result["call_id"], "capture");
                }
                ApiFormat::Anthropic => {
                    assert_eq!(result["role"], "user");
                    assert_eq!(result["content"][0]["type"], "tool_result");
                    assert_eq!(result["content"][0]["tool_use_id"], "capture");
                }
                _ => {
                    assert_eq!(result["role"], "user");
                    assert_eq!(result["parts"][0]["functionResponse"]["name"], "view_image");
                }
            }
            assert!(result.to_string().contains("Image attached"));
            assert_eq!(wire_count(&Value::Array(wire)), 0);
        }
    }

    #[test]
    fn projected_pixels_stay_within_budget_in_every_provider_wire_format() {
        for format in ["openai", "responses", "anthropic", "gemini"] {
            let mut user = user_images(&["a", "b"]);
            if format == "anthropic" {
                user.content = Some(json!([
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "a"}},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "b"}},
                    {"type": "text", "text": "Inspect these images."}
                ]).to_string());
            }
            let projected = project(vec![user, tool_images("a", 2)], Some(3)).unwrap();
            let wire = match format {
                "openai" => json!(OpenAIMessageConverter::convert_messages(projected)),
                "responses" => {
                    json!(OpenAIMessageConverter::convert_messages_to_responses_input(projected).1)
                }
                "anthropic" => json!(AnthropicMessageConverter::convert_messages(projected).1),
                _ => json!(GeminiMessageConverter::convert_messages(projected, "gemini-3-pro").1),
            };
            assert_eq!(wire_count(&wire), 3, "format={format}, wire={wire}");
        }
    }
}
