use super::context::project;
use crate::types::{AIConfig, Message, ModelRequestContext, ToolImageAttachment};
use crate::vision_tokens::{inline_image_dimensions, VisionTokenPolicy};
use base64::{engine::general_purpose::STANDARD, Engine};
use openbitfun_core_types::token_estimate::INPUT_SAFETY_RESERVE;
use serde_json::{json, Value};
use std::io::Cursor;

fn config() -> AIConfig {
    serde_json::from_value(json!({
        "name": "vision", "base_url": "http://localhost", "request_url": "http://localhost",
        "api_key": "synthetic", "model": "test-vl", "format": "openai",
        "context_window": 262144, "max_tokens": 16384,
        "inline_think_in_text": false, "skip_ssl_verify": false
    }))
    .unwrap()
}

fn with_remaining_input(
    config: &AIConfig,
    turn_start: usize,
    remaining: usize,
) -> ModelRequestContext {
    ModelRequestContext {
        image_turn_start: Some(turn_start),
        minimum_non_image_input_tokens: config.context_window as usize
            - config.max_tokens.unwrap() as usize
            - INPUT_SAFETY_RESERVE
            - remaining,
        ..Default::default()
    }
}

fn png(width: u32, height: u32) -> String {
    encode_png(image::GrayImage::new(width, height))
}

fn encode_png(pixels: image::GrayImage) -> String {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(pixels)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    STANDARD.encode(bytes.into_inner())
}

fn screenshot_with_small_text() -> String {
    let mut page = image::GrayImage::from_pixel(2048, 2048, image::Luma([255]));
    // Draw "Hi" in 15-pixel-high letters in the middle of the screenshot.
    for (y, row) in ["# #  #", "# #   ", "###  #", "# #  #", "# #  #"]
        .iter()
        .enumerate()
    {
        for (x, _) in row.bytes().enumerate().filter(|(_, c)| *c == b'#') {
            for dy in 0..3 {
                for dx in 0..3 {
                    page.put_pixel(
                        1024 + 3 * x as u32 + dx,
                        1024 + 3 * y as u32 + dy,
                        image::Luma([0]),
                    );
                }
            }
        }
    }
    encode_png(page)
}

fn uploaded(data: &str) -> Message {
    Message::user(
        json!([
            {"type": "text", "text": "Inspect this page."},
            {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{data}")}}
        ])
        .to_string(),
    )
}

fn tool_result(name: &str, data: &str) -> Message {
    let mut message = Message::user("Screenshot available.".into());
    message.role = "tool".into();
    message.name = Some(name.into());
    message.tool_call_id = Some("capture".into());
    message.tool_image_attachments = Some(vec![ToolImageAttachment {
        mime_type: "image/png".into(),
        data_base64: data.into(),
    }]);
    message
}

fn user_image_size(message: &Message) -> Option<(u32, u32)> {
    let parts: Vec<Value> = serde_json::from_str(message.content.as_ref()?).ok()?;
    let url = parts
        .iter()
        .find_map(|part| part["image_url"]["url"].as_str())?;
    inline_image_dimensions("image/png", url.strip_prefix("data:image/png;base64,")?)
}

#[test]
fn openai_preserves_literal_json_and_validates_explicit_image_parts() {
    use crate::providers::openai::OpenAIMessageConverter;
    let url = format!("data:image/png;base64,{}", png(4, 4));
    let image = json!({"type": "input_image", "image_url": url});
    for format in ["openai", "responses"] {
        let mut config = config();
        config.format = format.into();
        let mut mixed = [Message::user(
            json!([image, {"type":"unsupported"}]).to_string(),
        )];
        let original = mixed[0].content.clone();
        project(&mut mixed, &config, None, None, None).unwrap();
        assert_eq!(mixed[0].content, original);

        let mut unsupported = [Message::user(
            json!([image, {"type":"audio", "data":"audio"}]).to_string(),
        )];
        let error = project(&mut unsupported, &config, None, None, None).unwrap_err();
        assert!(error
            .to_string()
            .contains("Unsupported user content part at index 1"));
        assert!(!error.to_string().contains(&url));

        let mut supported = vec![Message::user(
            json!([image, {"type": "input_text", "text":"Inspect"}]).to_string(),
        )];
        project(&mut supported, &config, None, None, None).unwrap();
        if format == "openai" {
            let wire = OpenAIMessageConverter::convert_messages(supported);
            assert_eq!(wire[0]["content"][0]["image_url"]["url"], url);
            assert_eq!(wire[0]["content"][1]["text"], "Inspect");
        } else {
            let (_, wire) = OpenAIMessageConverter::convert_messages_to_responses_input(supported);
            assert_eq!(wire[0]["content"][0]["image_url"], url);
            assert_eq!(wire[0]["content"][1]["text"], "Inspect");
        }
    }
}

#[test]
fn omit_history_before_resizing_the_current_screenshot_and_preserve_originals() {
    let config = config();
    let data = screenshot_with_small_text();
    let original = vec![uploaded(&png(1536, 1536)), tool_result("view_image", &data)];
    let mut roomy = original.clone();
    let turn = ModelRequestContext {
        image_turn_start: Some(1),
        ..Default::default()
    };
    project(&mut roomy, &config, None, Some(&turn), None).unwrap();
    assert!(user_image_size(&roomy[0]).is_none());
    assert_eq!(
        roomy[1].tool_image_attachments.as_ref().unwrap()[0].data_base64,
        data
    );

    let mut limited = original.clone();
    let context = with_remaining_input(
        &config,
        1,
        VisionTokenPolicy::for_model("test-vl").estimate(2048, 2048) + 1000,
    );
    project(&mut limited, &config, None, Some(&context), None).unwrap();
    assert!(user_image_size(&limited[0]).is_none());
    assert!(limited[0]
        .content
        .as_deref()
        .unwrap()
        .contains("Inspect this page."));
    assert_eq!(limited[1].tool_call_id.as_deref(), Some("capture"));
    assert_eq!(
        limited[1].tool_image_attachments.as_ref().unwrap()[0].data_base64,
        data
    );
    let mut tight = original.clone();
    let remaining = 1200;
    let context = with_remaining_input(&config, 1, remaining);
    project(&mut tight, &config, None, Some(&context), None).unwrap();
    assert!(user_image_size(&tight[0]).is_none());
    let preview = &tight[1].tool_image_attachments.as_ref().unwrap()[0];
    let (width, height) =
        inline_image_dimensions(&preview.mime_type, &preview.data_base64).unwrap();
    assert_eq!(width, height);
    assert!((512..2048).contains(&width));
    assert!(VisionTokenPolicy::for_model(&config.model).estimate(width, height) <= remaining);
    assert_eq!(tight[1].tool_call_id.as_deref(), Some("capture"));

    let decoded = image::load_from_memory(&STANDARD.decode(&preview.data_base64).unwrap())
        .unwrap()
        .into_luma8();
    let center = image::imageops::crop_imm(&decoded, width / 2, height / 2, 10, 10).to_image();
    assert!(
        center.pixels().any(|p| p[0] < 96),
        "the preview retains the dark letters"
    );
    assert!(
        center.pixels().any(|p| p[0] > 230),
        "the preview retains the white background"
    );
    assert_eq!(user_image_size(&original[0]), Some((1536, 1536)));
    assert_eq!(
        original[1].tool_image_attachments.as_ref().unwrap()[0].data_base64,
        data
    );
}

#[test]
fn earlier_tool_round_yields_before_the_latest_parallel_results() {
    let config = config();
    let data = png(1024, 1024);
    let call = |id: &str| crate::ToolCall {
        id: id.into(),
        name: "view_image".into(),
        arguments: json!({}),
        raw_arguments: None,
    };
    let result = |id: &str| {
        let mut result = tool_result("view_image", &data);
        result.tool_call_id = Some(id.into());
        result
    };
    let mut messages = vec![
        Message::user("Compare the screenshots.".into()),
        Message::assistant_with_tools(vec![call("old")]),
        result("old"),
        Message::assistant_with_tools(vec![call("new-a"), call("new-b")]),
        result("new-a"),
        result("new-b"),
    ];
    let pair = 2 * VisionTokenPolicy::for_model(&config.model).estimate(1024, 1024);
    let context = with_remaining_input(&config, 0, pair + 100);
    project(&mut messages, &config, None, Some(&context), None).unwrap();
    assert!(messages[2].tool_image_attachments.is_none());
    assert_eq!(messages[2].tool_call_id.as_deref(), Some("old"));
    for message in &messages[4..] {
        assert_eq!(
            message.tool_image_attachments.as_ref().unwrap()[0].data_base64,
            data
        );
    }
}

#[test]
fn text_only_tool_round_can_omit_the_upload_and_last_rounds_screenshot() {
    let config = config();
    let data = png(1024, 1024);
    let batch = |id: &str, tool: &str| {
        Message::assistant_with_tools(vec![crate::ToolCall {
            id: id.into(),
            name: tool.into(),
            arguments: json!({}),
            raw_arguments: None,
        }])
    };
    let mut text_result = Message::user("Done".into());
    text_result.role = "tool".into();
    text_result.name = Some("ExecCommand".into());
    text_result.tool_call_id = Some("exec".into());
    let mut messages = vec![
        uploaded(&data),
        batch("capture", "view_image"),
        tool_result("view_image", &data),
        batch("exec", "ExecCommand"),
        text_result,
    ];
    // Both earlier images have already been seen. This round fits placeholders only.
    let context = with_remaining_input(&config, 0, 100);
    project(&mut messages, &config, None, Some(&context), None).unwrap();
    assert!(user_image_size(&messages[0]).is_none());
    assert!(messages[2].tool_image_attachments.is_none());
    assert_eq!(messages[2].tool_call_id.as_deref(), Some("capture"));
    assert_eq!(messages[4].content.as_deref(), Some("Done"));
}

#[test]
fn openai_low_detail_fits_at_the_selected_models_base_cost() {
    let mut config = config();
    config.model = "gpt-4o".into();
    let data = png(1024, 1024);
    let url = format!("data:image/png;base64,{data}");
    let context = with_remaining_input(&config, 0, 85);
    for part in [
        json!({"type":"image_url","image_url":{"url": url,"detail":"low"}}),
        json!({"type":"input_image","image_url":url,"detail":"low"}),
    ] {
        let content = json!([part]).to_string();
        let mut messages = [Message::user(content.clone())];
        project(&mut messages, &config, None, Some(&context), None).unwrap();
        assert_eq!(messages[0].content.as_deref(), Some(content.as_str()));
    }
    let extra = json!({"model":"gpt-4o"});
    config.model = "test-vl".into();
    let mut overridden = [Message::user(
        json!([{"type":"image_url","image_url":{"url":url,"detail":"low"}}]).to_string(),
    )];
    let error = project(&mut overridden, &config, None, Some(&context), Some(&extra)).unwrap_err();
    assert!(error
        .to_string()
        .contains("custom request changes the model"));
    config.custom_request_body_mode = Some("trim".into());
    assert!(project(&mut overridden, &config, None, Some(&context), Some(&extra)).is_err());
    config.model = "gpt-4o".into();
    let mut auto = [uploaded(&data)];
    let error = project(&mut auto, &config, None, Some(&context), None).unwrap_err();
    assert!(error.to_string().contains("Crop the relevant area"));
    assert_eq!(
        error
            .downcast_ref::<openbitfun_core_types::errors::AiProviderError>()
            .unwrap()
            .category,
        openbitfun_core_types::ErrorCategory::InvalidRequest
    );
}

#[test]
fn gemini_uses_global_resolution_for_projected_images_and_override_parts_on_the_wire() {
    let mut config = config();
    config.model = "gemini-3.6-flash".into();
    config.format = "gemini".into();
    let context = with_remaining_input(&config, 0, 600);
    let data = png(32, 32);
    let messages = vec![Message::user(
        json!([{
            "inlineData": {"mimeType": "image/png", "data": data},
            // Normal message conversion drops this field.
            "mediaResolution": {"level": "MEDIA_RESOLUTION_LOW"}
        }])
        .to_string(),
    )];
    assert!(project(&mut messages.clone(), &config, None, Some(&context), None).is_err());
    let global = json!({"generationConfig": {"mediaResolution": "MEDIA_RESOLUTION_MEDIUM"}});
    project(
        &mut messages.clone(),
        &config,
        None,
        Some(&context),
        Some(&global),
    )
    .unwrap();

    // A raw contents override goes to the provider as written. 1600 input
    // tokens allow the default resolution but not an ultra-high image part.
    config.context_window = 6000;
    config.max_tokens = Some(2400);
    let mut raw = json!({
        "generationConfig": {"mediaResolution": "MEDIA_RESOLUTION_MEDIUM"},
        "contents": [{"role":"user", "parts": [{
            "fileData": {"fileUri":"gs://test/screenshot.png", "mimeType":"image/png"},
            "mediaResolution": {"level":"MEDIA_RESOLUTION_ULTRA_HIGH"}
        }]}]
    });
    assert!(super::context::validate_override(
        &crate::AIClient::new(config.clone()),
        &[],
        None,
        None,
        Some(&raw)
    )
    .is_err());
    raw["contents"][0]["parts"][0]
        .as_object_mut()
        .unwrap()
        .remove("mediaResolution");
    assert!(super::context::validate_override(
        &crate::AIClient::new(config.clone()),
        &[],
        None,
        None,
        Some(&raw)
    )
    .unwrap());
}

#[test]
fn current_upload_and_tool_share_remaining_visual_tokens() {
    let config = config();
    let data = png(1920, 1080);
    let mut messages = vec![uploaded(&data), tool_result("view_image", &data)];
    let remaining = 3000;
    let context = with_remaining_input(&config, 0, remaining);
    project(&mut messages, &config, None, Some(&context), None).unwrap();
    let user = user_image_size(&messages[0]).expect("current user image kept");
    let tool_image = &messages[1]
        .tool_image_attachments
        .as_ref()
        .expect("current tool image kept")[0];
    let tool = inline_image_dimensions(&tool_image.mime_type, &tool_image.data_base64).unwrap();
    assert!(user.0 >= 512 && tool.0 >= 512);
    assert!(
        VisionTokenPolicy::for_model("test-vl").estimate(user.0, user.1)
            + VisionTokenPolicy::for_model("test-vl").estimate(tool.0, tool.1)
            <= remaining
    );
    assert_eq!(messages[1].tool_call_id.as_deref(), Some("capture"));
}

#[test]
fn output_reserve_messages_and_tool_definitions_share_the_input_window() {
    let config = config();
    let data = png(640, 640);
    let mut messages = vec![uploaded(&data)];
    let context = with_remaining_input(&config, 0, 1);
    assert!(project(&mut messages, &config, None, Some(&context), None).is_err());

    let mut small = config.clone();
    small.context_window = 18000;
    small.max_tokens = Some(4000);
    let mut messages = vec![
        Message::user("review these details ".repeat(10000)),
        uploaded(&data),
    ];
    assert!(project(&mut messages, &small, None, None, None).is_err());

    let tools = [crate::types::ToolDefinition {
        name: "lookup".into(),
        description: "Look up one of the available entries".into(),
        parameters: json!({"type":"string", "enum": (0..5000).map(|index| format!("entry-{index}")).collect::<Vec<_>>() }),
    }];
    let mut image_only = [uploaded(&data)];
    project(&mut image_only, &small, None, None, None).unwrap();
    assert!(project(&mut image_only, &small, Some(&tools), None, None).is_err());
}

#[test]
fn current_computer_use_frame_keeps_its_coordinate_system() {
    let config = config();
    let data = png(1920, 1080);
    let mut messages = vec![tool_result("ComputerUse", &data)];
    project(&mut messages, &config, None, None, None).unwrap();
    let original = &messages[0].tool_image_attachments.as_ref().unwrap()[0];
    assert_eq!(original.data_base64, data);
    assert_eq!(
        inline_image_dimensions(&original.mime_type, &original.data_base64),
        Some((1920, 1080))
    );
    let context = with_remaining_input(&config, 0, 1200);
    assert!(project(&mut messages, &config, None, Some(&context), None).is_err());
    assert_eq!(
        messages[0].tool_image_attachments.as_ref().unwrap()[0].data_base64,
        data
    );
}

#[test]
fn resized_native_image_parts_survive_anthropic_and_gemini_serialization() {
    use crate::providers::{
        anthropic::message_converter::AnthropicMessageConverter,
        gemini::message_converter::GeminiMessageConverter,
    };

    let data = png(1200, 900);
    let remaining = 1000;
    for (format, part) in [
        (
            "anthropic",
            json!({"type":"image", "source":{"type":"base64","media_type":"image/png","data":data}}),
        ),
        (
            "gemini",
            json!({"inline_data":{"mime_type":"image/png","data":data}}),
        ),
        (
            "gemini",
            json!({"inlineData":{"mimeType":"image/png","data":data}}),
        ),
    ] {
        let mut config = config();
        config.format = format.into();
        let context = with_remaining_input(&config, 0, remaining);
        let mut messages = vec![Message::user(json!([part]).to_string())];
        project(&mut messages, &config, None, Some(&context), None).unwrap();
        let (encoded, mime_key) = if format == "anthropic" {
            let (_, wire) = AnthropicMessageConverter::convert_messages(messages);
            (wire[0]["content"][0]["source"].clone(), "media_type")
        } else {
            let (_, wire) = GeminiMessageConverter::convert_messages(messages, &config.model);
            (wire[0]["parts"][0]["inlineData"].clone(), "mimeType")
        };
        let mime = encoded[mime_key].as_str().unwrap();
        assert_eq!(mime, "image/png");
        let (w, h) = inline_image_dimensions(mime, encoded["data"].as_str().unwrap()).unwrap();
        assert!(w >= 512 && w < 1200);
        assert!(VisionTokenPolicy::for_model("test-vl").estimate(w, h) <= remaining);
    }
}

#[tokio::test]
async fn selected_model_window_controls_pixels_on_the_wire() {
    use axum::{routing::post, Json, Router};
    use std::sync::{Arc, Mutex};

    let observed = Arc::new(Mutex::new(Vec::new()));
    let received = observed.clone();
    let app = Router::new().route("/chat/completions", post(move |Json(body): Json<Value>| {
        let received = received.clone();
        async move {
            let data = body["messages"][0]["content"].as_array().unwrap().iter()
                .find_map(|part| part["image_url"]["url"].as_str()).unwrap()
                .strip_prefix("data:image/png;base64,").unwrap();
            let dimensions = inline_image_dimensions("image/png", data).unwrap();
            received.lock().unwrap().push(dimensions);
            let chunk = json!({"id":"test","object":"chat.completion.chunk","created":1,"model":"test-vl",
                "choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]});
            ([("content-type", "text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/chat/completions", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut roomy = config();
    roomy.base_url = url.clone();
    roomy.request_url = url;
    let mut limited = roomy.clone();
    limited.context_window = 128_000;
    let context = with_remaining_input(&limited, 0, 1500);
    let impossible = with_remaining_input(&limited, 0, 1);
    let messages = vec![uploaded(&png(1920, 1080))];
    for selected in [roomy, limited.clone()] {
        crate::AIClient::new(selected)
            .send_message_once_with_trace_and_request_context(
                messages.clone(),
                None,
                Some(context.clone()),
                None,
            )
            .await
            .unwrap();
    }
    let result = crate::AIClient::new(limited)
        .send_message_once_with_trace_and_request_context(messages, None, Some(impossible), None)
        .await;
    assert!(result.is_err());
    server.abort();
    let requests = observed.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "the infeasible third request must fail before HTTP"
    );
    assert_eq!(requests[0], (1920, 1080));
    let (width, height) = requests[1];
    assert!(
        width >= 512 && VisionTokenPolicy::for_model("test-vl").estimate(width, height) <= 1500
    );
}

#[tokio::test]
async fn later_http_rounds_send_only_fresh_pixels_under_a_gateway_body_limit() {
    use axum::{body::Bytes, extract::DefaultBodyLimit, routing::post, Router};
    use std::sync::{Arc, Mutex};

    // Incompressible screenshots of the same dimensions have comparable byte
    // sizes. The gateway can accept one image with framing, but never both.
    let screenshot = |mut seed: u32| {
        encode_png(image::GrayImage::from_fn(256, 256, |_, _| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            image::Luma([(seed >> 24) as u8])
        }))
    };
    let a = screenshot(1);
    let b = screenshot(2);
    let limit = a.len().max(b.len()) + 8192;
    assert!(a.len() + b.len() > limit);
    let requests = Arc::new(Mutex::new(Vec::<Bytes>::new()));
    let captured = requests.clone();
    let router = Router::new().route("/chat/completions", post(move |body: Bytes| {
        let captured = captured.clone();
        async move {
            captured.lock().unwrap().push(body);
            let chunk = json!({"id":"test","object":"chat.completion.chunk","created":1,"model":"test-vl",
                "choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]});
            ([("content-type", "text/event-stream")], format!("data: {chunk}\n\ndata: [DONE]\n\n"))
        }
    })).layer(DefaultBodyLimit::max(limit));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/chat/completions", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut config = config();
    config.request_url = url.clone();
    config.base_url = url;
    let client = crate::AIClient::new(config);
    let mut history = vec![Message::user("Read screenshot A".into())];
    for next in [
        Some(("a", &a)),
        None,
        Some(("b", &b)),
        Some(("a-again", &a)),
    ] {
        if let Some((id, data)) = next {
            history.push(Message::assistant_with_tools(vec![crate::ToolCall {
                id: id.into(),
                name: "view_image".into(),
                arguments: json!({"path":id}),
                raw_arguments: None,
            }]));
            let mut result = tool_result("view_image", data);
            result.tool_call_id = Some(id.into());
            history.push(result);
        } else {
            history.push(Message::assistant("A reviewed".into()));
            history.push(Message::user("Continue".into()));
        }
        client
            .send_message_once_with_trace_and_request_context(history.clone(), None, None, None)
            .await
            .unwrap();
    }
    let calls = ["batch-a", "batch-b"]
        .into_iter()
        .map(|id| crate::ToolCall {
            id: id.into(),
            name: "view_image".into(),
            arguments: json!({"path":id}),
            raw_arguments: None,
        })
        .collect();
    let mut batch = vec![
        Message::user("Read both screenshots".into()),
        Message::assistant_with_tools(calls),
    ];
    for (id, pixels) in [("batch-a", &a), ("batch-b", &b)] {
        let mut result = tool_result("view_image", pixels);
        result.tool_call_id = Some(id.into());
        batch.push(result);
    }
    client
        .send_message_once_with_trace_and_request_context(batch, None, None, None)
        .await
        .expect("a 413 must allow a smaller tool-image batch to succeed");
    server.abort();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 5);
    for (wire, (expected_a, expected_b)) in requests.iter().zip([(1, 0), (0, 0), (0, 1), (1, 0)]) {
        assert!(wire.len() <= limit);
        let json = std::str::from_utf8(wire).unwrap();
        assert_eq!(json.matches(a.as_str()).count(), expected_a);
        assert_eq!(json.matches(b.as_str()).count(), expected_b);
    }
    let recovered = std::str::from_utf8(&requests[4]).unwrap();
    assert!(!recovered.contains(&a));
    assert!(recovered.contains(&b));
    assert!(recovered.contains("omitted to fit the request body"));
    let body: Value = serde_json::from_str(recovered).unwrap();
    let tool_ids: Vec<_> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|part| part["tool_call_id"].as_str())
        .collect();
    assert_eq!(tool_ids, ["batch-a", "batch-b"]);
    assert!(requests[1].len() < 8192);
    assert_eq!(
        history[2].tool_image_attachments.as_ref().unwrap()[0].data_base64,
        a
    );
}

#[test]
fn custom_image_payload_must_fit_the_actual_output_reserve() {
    let mut config = config();
    config.context_window = 32_000;
    config.max_tokens = Some(8_000);
    let native: Value =
        serde_json::from_str(uploaded(&png(1024, 1024)).content.as_deref().unwrap()).unwrap();
    let mut body = json!({"messages": [{"role": "user", "content": native}]});
    super::context::validate_override(
        &crate::AIClient::new(config.clone()),
        &[],
        None,
        None,
        Some(&body),
    )
    .unwrap();
    body["max_completion_tokens"] = json!(21_999);
    let error = super::context::validate_override(
        &crate::AIClient::new(config.clone()),
        &[],
        None,
        None,
        Some(&body),
    )
    .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<openbitfun_core_types::errors::AiProviderError>()
            .unwrap()
            .category,
        openbitfun_core_types::ErrorCategory::InvalidRequest
    );
    config.format = "responses".into();
    let native = body["messages"].take();
    let responses = json!({"input": native, "max_output_tokens": 20_500});
    assert!(super::context::validate_override(
        &crate::AIClient::new(config.clone()),
        &[],
        None,
        None,
        Some(&responses)
    )
    .unwrap());
    let system = Message::system("long instruction ".repeat(500));
    assert!(super::context::validate_override(
        &crate::AIClient::new(config.clone()),
        &[system],
        None,
        None,
        Some(&responses)
    )
    .is_err());
    config.format = "openai".into();
    body["messages"] = json!([{"role":"user","content":"hello"}]);
    assert!(super::context::validate_override(
        &crate::AIClient::new(config.clone()),
        &[],
        None,
        None,
        Some(&body)
    )
    .is_ok());
}

#[test]
fn deferred_tool_preserves_only_coordinate_bound_frames() {
    let config = config();
    let pixels = png(1800, 900);
    let budget = with_remaining_input(&config, 0, 1200);
    for (name, should_fit) in [
        ("ComputerUse", false),
        ("view_image", true),
        ("DocumentPreview", true),
    ] {
        let call = crate::ToolCall {
            id: "capture".into(),
            name: "CallDeferredTool".into(),
            arguments: json!({"tool_name":name}),
            raw_arguments: None,
        };
        let mut messages = vec![
            Message::assistant_with_tools(vec![call]),
            tool_result("CallDeferredTool", &pixels),
        ];
        let result = project(&mut messages, &config, None, Some(&budget), None);
        assert_eq!(result.is_ok(), should_fit, "{name}: {result:?}");
    }
}

#[test]
fn anthropic_preflight_uses_the_effective_output_override() {
    let mut config = config();
    config.format = "anthropic".into();
    config.context_window = 32_000;
    config.max_tokens = Some(8_000);
    let extra = json!({"max_tokens":1024});
    let message = Message::user(
        json!([
            {"type":"text","text":"some words ".repeat(4500)},
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":png(512,512)}}
        ])
        .to_string(),
    );
    project(&mut [message.clone()], &config, None, None, Some(&extra)).unwrap();
    config.custom_request_body_mode = Some("trim".into());
    let error = project(&mut [message], &config, None, None, Some(&extra)).unwrap_err();
    assert!(error.to_string().contains("14000 available"), "{error}");
}

#[test]
fn anthropic_file_images_participate_in_preflight() {
    let mut config = config();
    config.format = "anthropic".into();
    config.context_window = 32_000;
    config.max_tokens = Some(8_000);
    let context = with_remaining_input(&config, 0, 1);
    let part = json!({"type":"image","source":{"type":"file","file_id":"file-fixture"}});
    let message = Message::user(json!([part.clone()]).to_string());
    assert!(project(&mut [message], &config, None, Some(&context), None).is_err());

    let extra = json!({"messages":[{"role":"user","content":[part]}],"max_tokens":22_000});
    let client = crate::AIClient::new(config);
    assert!(super::context::validate_override(&client, &[], None, None, Some(&extra)).is_err());
}

#[test]
fn responses_does_not_count_an_image_against_only_part_of_an_external_history() {
    let mut config = config();
    config.format = "responses".into();
    let client = crate::AIClient::new(config);
    let mut input = json!({"input":[{"role":"user","content":[{"type":"input_image",
        "image_url":format!("data:image/png;base64,{}", png(512,512))}]}]});
    assert!(super::context::validate_override(&client, &[], None, None, Some(&input)).unwrap());
    for (key, value) in [
        ("previous_response_id", "resp_fixture"),
        ("conversation", "conv_fixture"),
    ] {
        input[key] = json!(value);
        let error =
            super::context::validate_override(&client, &[], None, None, Some(&input)).unwrap_err();
        assert!(error.to_string().contains("cannot be budgeted locally"));
        input.as_object_mut().unwrap().remove(key);
    }
}

#[cfg(feature = "subscription-auth")]
#[tokio::test]
async fn codex_preflight_ignores_output_fields_removed_by_the_subscription_route() {
    let mut config = config();
    config.format = "responses".into();
    config.request_url = "https://chatgpt.com/backend-api/codex/responses".into();
    config.context_window = 32_000;
    config.max_tokens = Some(8_000);
    let client = crate::AIClient::new(config)
        .with_subscription_provider(crate::subscription_auth::SubscriptionProvider::Codex);
    client
        .prepare_message_inputs(
            vec![uploaded(&png(512, 512))],
            None,
            None,
            Some(&json!({"max_output_tokens":22_000})),
        )
        .await
        .unwrap();
}
