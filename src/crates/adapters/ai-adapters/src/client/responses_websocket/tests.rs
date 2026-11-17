use super::*;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Server {
    url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    connections: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn server(reject_second: bool, interrupt: bool) -> Server {
    start_server(reject_second, interrupt, false).await
}

async fn start_server(reject_second: bool, interrupt: bool, stall_first: bool) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/responses", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let connections = Arc::new(AtomicUsize::new(0));
    let received = requests.clone();
    let connected = connections.clone();
    let task = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            connected.fetch_add(1, Ordering::SeqCst);
            let received = received.clone();
            tasks.spawn(async move {
                    let mut socket = tokio_tungstenite::accept_hdr_async(tcp, |request: &tungstenite::handshake::server::Request, response| {
                        assert_eq!(request.uri().path(), "/responses");
                        assert_eq!(request.headers()["authorization"], "Bearer synthetic");
                        Ok(response)
                    }).await.unwrap();
                    while let Some(Ok(Message::Text(text))) = socket.next().await {
                        let request: Value = serde_json::from_str(&text).unwrap();
                        let number = { let mut requests = received.lock().unwrap(); requests.push(request); requests.len() };
                        if reject_second && number == 2 {
                            socket.send(Message::Text(json!({"type":"error","status":400,"error":{"code":"previous_response_not_found","message":"Expired"}}).to_string().into())).await.unwrap();
                            continue;
                        }
                        let item = json!({"type":"message","id":format!("msg_{number}"),"status":"completed","role":"assistant","content":[{"type":"output_text","text":"Hello","annotations":[]}]});
                        for event in [
                            json!({"type":"response.created","response":{"id":format!("resp_{number}"),"model":"test"}}),
                            json!({"type":"response.output_text.delta","delta":"Hello"}),
                            json!({"type":"response.output_item.done","output_index":0,"item":item}),
                        ] {
                            if socket.send(Message::Text(event.to_string().into())).await.is_err() { return; }
                            if stall_first && number == 1 && event["type"] == "response.created" {
                                futures::future::pending::<()>().await;
                            }
                        }
                        if interrupt { let _ = socket.close(None).await; return; }
                        let end = json!({"type":"response.completed","response":{"id":format!("resp_{number}"),"output":[item],"usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}}});
                        if socket.send(Message::Text(end.to_string().into())).await.is_err() { return; }
                    }
                });
        }
    });
    Server {
        url,
        requests,
        connections,
        task,
    }
}

fn client(server: &Server, incremental: bool) -> AIClient {
    AIClient::new(serde_json::from_value(json!({
            "name":"fixture","base_url":server.url,"request_url":server.url,"api_key":"synthetic",
            "model":"test","format":"responses","context_window":128000,"inline_think_in_text":false,
            "skip_ssl_verify":false,"responses":{"transport":"websocket","context":if incremental {"incremental"} else {"full"}}
        })).unwrap())
}

fn context(name: &str) -> ModelRequestContext {
    ModelRequestContext {
        conversation_scope: Some(Arc::new(name.into())),
        ..Default::default()
    }
}

fn body(next: bool) -> Value {
    let mut input =
        vec![json!({"type":"message","role":"user","content":[{"type":"input_text","text":"Hi"}]})];
    if next {
        input.push(json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello"}]}));
        input.push(json!({"type":"message","role":"user","content":[{"type":"input_text","text":"Continue"}]}));
    }
    json!({"model":"test","store":false,"stream":true,"input":input})
}

async fn drain(mut response: StreamResponse) -> Result<()> {
    while let Some(chunk) = response.stream.next().await {
        chunk?;
    }
    Ok(())
}

#[tokio::test]
async fn websocket_incremental_reuses_connection_and_sends_only_new_input() {
    let server = server(false, false).await;
    let client = client(&server, true);
    let context = context("main");
    drain(
        send(&client, body(false), Some(context.clone()), None)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    drain(
        send(&client, body(true), Some(context), None)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["previous_response_id"], "resp_1");
    assert_eq!(requests[1]["input"].as_array().unwrap().len(), 1);
    assert_eq!(requests[1]["store"], false);
    assert!(requests[1].get("stream").is_none());
    assert_eq!(server.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn websocket_full_mode_reuses_transport_without_continuation() {
    let server = server(false, false).await;
    let client = client(&server, false);
    let context = context("main");
    drain(
        send(&client, body(false), Some(context.clone()), None)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    drain(
        send(&client, body(true), Some(context), None)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    let requests = server.requests.lock().unwrap();
    assert!(requests[1].get("previous_response_id").is_none());
    assert_eq!(requests[1]["input"].as_array().unwrap().len(), 3);
    assert_eq!(server.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejected_continuation_rebuilds_once_without_enabling_storage() {
    let server = server(true, false).await;
    let client = client(&server, true);
    let context = context("main");
    drain(
        send(&client, body(false), Some(context.clone()), None)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    drain(
        send(&client, body(true), Some(context), None)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[1].get("previous_response_id").is_some());
    assert!(requests[2].get("previous_response_id").is_none());
    assert_eq!(requests[2]["input"].as_array().unwrap().len(), 3);
    assert_eq!(requests[2]["store"], false);
}

#[tokio::test]
async fn conversations_and_changed_client_identity_do_not_share_checkpoints() {
    let server = server(false, false).await;
    let client = client(&server, true);
    let main = context("main");
    drain(
        send(&client, body(false), Some(main.clone()), None)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    drain(
        send(&client, body(true), Some(context("child")), None)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    let mut changed = client.clone();
    changed.config.model = "another".into();
    drain(send(&changed, body(true), Some(main), None).await.unwrap())
        .await
        .unwrap();
    assert!(server
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|body| body.get("previous_response_id").is_none()));
    assert_eq!(server.connections.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn interrupted_response_is_not_replayed_by_aggregate_retry() {
    let server = server(false, true).await;
    let client = client(&server, true);
    let error = client
        .send_message_with_trace_and_request_context(
            vec![crate::types::Message::user("Hi".into())],
            None,
            Some(context("main")),
            None,
        )
        .await
        .unwrap_err();
    assert!(
        error
            .downcast_ref::<AiProviderError>()
            .unwrap()
            .delivery_uncertain
    );
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[test]
fn changed_history_and_request_options_start_new_chains() {
    let full = websocket_body(body(false)).unwrap();
    let previous = Checkpoint {
        id: "r1".into(),
        request: full,
        output: vec![body(true)["input"][1].clone()],
    };
    let mut next = websocket_body(body(true)).unwrap();
    assert!(incremental(&next, &previous).is_some());
    next["instructions"] = json!("Changed");
    assert!(incremental(&next, &previous).is_none());
    next.as_object_mut().unwrap().remove("instructions");
    next["input"][0]["content"][0]["text"] = json!("Edited");
    assert!(incremental(&next, &previous).is_none());
    assert!(incremental(&websocket_body(body(false)).unwrap(), &previous).is_none());
}

#[test]
fn managed_websocket_fields_cannot_be_overridden() {
    for (key, value) in [
        ("previous_response_id", json!("foreign")),
        ("conversation", json!("foreign")),
        ("store", json!(true)),
        ("background", json!(true)),
    ] {
        let mut body = body(false);
        body[key] = value;
        assert!(websocket_body(body).is_err(), "{key}");
    }
}
#[tokio::test]
async fn actual_message_conversion_preserves_incremental_replay() {
    use crate::types::Message as ChatMessage;
    use openbitfun_core_types::ModelResponseReplay;
    let server = server(false, false).await;
    let client = client(&server, true);
    let context = context("actual-message-loop");
    let user = ChatMessage::user("Hi".into());
    let mut response = client
        .send_message_stream_once_with_request_context(
            vec![user.clone()],
            None,
            Some(context.clone()),
            None,
        )
        .await
        .unwrap();
    let mut text = String::new();
    let mut replay = None;
    while let Some(chunk) = response.stream.next().await {
        let chunk = chunk.unwrap();
        if let Some(delta) = chunk.text {
            text.push_str(&delta);
        }
        if let Some(capture) = chunk.model_response_replay {
            replay = Some(capture);
        }
    }
    let capture = replay.expect("completed output must be replayable without opaque reasoning");
    let mut assistant = ChatMessage::assistant(text);
    assistant.model_response_replay = Some(ModelResponseReplay {
        protocol: capture.protocol,
        items: capture.items,
        channel_origin: None,
        channel_fingerprint: None,
        account_origin: None,
    });
    drain(
        client
            .send_message_stream_once_with_request_context(
                vec![user, assistant, ChatMessage::user("Continue".into())],
                None,
                Some(context),
                None,
            )
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests[1]["previous_response_id"], "resp_1");
    assert_eq!(requests[1]["input"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn cancelling_stream_discards_socket_and_checkpoint() {
    let server = start_server(false, false, true).await;
    let client = client(&server, true);
    let context = context("cancelled");
    let response = send(&client, body(false), Some(context.clone()), None)
        .await
        .unwrap();
    drop(response);
    tokio::time::timeout(Duration::from_secs(2), async {
        drain(
            send(&client, body(true), Some(context), None)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
    })
    .await
    .unwrap();
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].get("previous_response_id").is_none());
    assert_eq!(server.connections.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn metadata_does_not_satisfy_ttft_and_timeout_is_not_replayed() {
    let server = start_server(false, false, true).await;
    let mut client = client(&server, true);
    client.stream_options.ttft_timeout = Some(Duration::from_millis(100));
    let error = client
        .send_message_with_trace_and_request_context(
            vec![crate::types::Message::user("Hi".into())],
            None,
            Some(context("ttft")),
            None,
        )
        .await
        .unwrap_err();
    assert!(
        error
            .downcast_ref::<AiProviderError>()
            .unwrap()
            .delivery_uncertain
    );
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[derive(Debug)]
struct Authorization(std::sync::atomic::AtomicBool);
impl super::super::ModelRequestAuthorization for Authorization {
    fn validate(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            if self.0.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(invalid("Revoked fixture"))
            }
        })
    }
}

#[tokio::test]
async fn reused_connection_rechecks_host_authorization() {
    let server = server(false, false).await;
    let guard = Arc::new(Authorization(std::sync::atomic::AtomicBool::new(true)));
    let client = client(&server, true).with_request_authorization(guard.clone());
    let context = context("revoked");
    drain(
        send(&client, body(false), Some(context.clone()), None)
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    guard.0.store(false, Ordering::SeqCst);
    assert!(send(&client, body(true), Some(context), None)
        .await
        .err()
        .unwrap()
        .to_string()
        .contains("Revoked fixture"));
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[derive(Default)]
struct Trace {
    attempts: Mutex<Vec<ModelExchangeRequestAttempt>>,
    failures: Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl crate::trace::ModelExchangeTraceSink for Trace {
    async fn request_attempt_started(
        &self,
        attempt: &ModelExchangeRequestAttempt,
    ) -> Option<crate::trace::ModelExchangeRequestTraceHandle> {
        let mut attempts = self.attempts.lock().unwrap();
        attempts.push(attempt.clone());
        Some(crate::trace::ModelExchangeRequestTraceHandle {
            trace_id: attempts.len().to_string(),
        })
    }
    async fn request_attempt_failed(
        &self,
        _: Option<&crate::trace::ModelExchangeRequestTraceHandle>,
        error: &str,
    ) {
        self.failures.lock().unwrap().push(error.into());
    }
    async fn request_attempt_completed(
        &self,
        _: &crate::trace::ModelExchangeRequestTraceHandle,
        _: &crate::trace::ModelExchangeResponseTrace,
    ) {
    }
}

#[tokio::test]
async fn recovery_traces_each_wire_payload_and_returns_the_accepted_attempt() {
    let server = server(true, false).await;
    let client = client(&server, true);
    let context = context("traced");
    let sink = Arc::new(Trace::default());
    let trace = ModelExchangeTraceConfig::new(sink.clone(), true);
    drain(
        send(
            &client,
            body(false),
            Some(context.clone()),
            Some(trace.clone()),
        )
        .await
        .unwrap(),
    )
    .await
    .unwrap();
    let response = send(&client, body(true), Some(context), Some(trace))
        .await
        .unwrap();
    assert_eq!(response.trace_handle.as_ref().unwrap().trace_id, "3");
    drain(response).await.unwrap();
    let attempts = sink.attempts.lock().unwrap();
    assert_eq!(
        attempts
            .iter()
            .map(|a| a.attempt_number)
            .collect::<Vec<_>>(),
        vec![1, 1, 2]
    );
    assert_eq!(
        attempts
            .iter()
            .map(|a| a.request_body.clone().unwrap())
            .collect::<Vec<_>>(),
        *server.requests.lock().unwrap()
    );
    assert_eq!(
        *sink.failures.lock().unwrap(),
        vec!["previous_response_not_found"]
    );
}

#[test]
fn invalid_transport_combinations_fail_before_network() {
    let config = serde_json::json!({"name":"invalid","base_url":"http://unused.invalid","request_url":"http://unused.invalid/responses",
            "api_key":"synthetic","model":"test","format":"responses","context_window":128000,"inline_think_in_text":false,"skip_ssl_verify":false,
            "responses":{"transport":"http","context":"incremental"}});
    let mut client = AIClient::new(serde_json::from_value(config).unwrap());
    assert!(validate(&client).is_err());
    client.config.responses.as_mut().unwrap().transport = ResponsesTransport::Websocket;
    client.config.format = "anthropic".into();
    assert!(validate(&client).is_err());
}

#[test]
fn tool_results_follow_exact_reasoning_phase_and_call_replay() {
    use crate::providers::openai::OpenAIMessageConverter;
    use crate::types::{Message as ChatMessage, ToolCall};
    use openbitfun_core_types::ModelResponseReplay;
    for content in [
        None,
        Some(json!([])),
        Some(json!([
            {"type":"reasoning_text", "text":" first\n"},
            {"type":"reasoning_text", "text":""},
            {"type":"reasoning_text", "text":"second"}
        ])),
    ] {
        let user = ChatMessage::user("Inspect repository".into());
        let mut output = vec![
            json!({"type":"reasoning","id":"rs1","status":"completed","content":[],"summary":[{"type":"summary_text","text":"Checking"}],"encrypted_content":"opaque"}),
            json!({"type":"message","id":"m1","role":"assistant","phase":"commentary","status":"completed","content":[{"type":"output_text","text":"Reading file.","annotations":[]}]}),
            json!({"type":"function_call","id":"fc1","call_id":"call1","status":"completed","name":"read","arguments":"{ \"path\": \"a.rs\" }"}),
        ];
        match content {
            Some(content) => output[0]["content"] = content,
            None => {
                output[0].as_object_mut().unwrap().remove("content");
            }
        }
        let capture =
            crate::stream::types::responses::parse_responses_replay_capture(&output).unwrap();
        let mut assistant = ChatMessage::assistant("Reading file.".into());
        assistant.tool_calls = Some(vec![ToolCall {
            id: "call1".into(),
            name: "read".into(),
            arguments: json!({"path":"a.rs"}),
            raw_arguments: None,
        }]);
        assistant.model_response_replay = Some(ModelResponseReplay {
            protocol: capture.protocol,
            items: capture.items,
            channel_origin: None,
            channel_fingerprint: None,
            account_origin: None,
        });
        // Exercise the persisted replay boundary before rebuilding the next input.
        assistant.model_response_replay =
            serde_json::from_value(serde_json::to_value(&assistant.model_response_replay).unwrap())
                .unwrap();
        let mut tool_result = ChatMessage::user("file contents".into());
        tool_result.role = "tool".into();
        tool_result.tool_call_id = Some("call1".into());
        let (_, first) =
            OpenAIMessageConverter::convert_messages_to_responses_input(vec![user.clone()]);
        let (_, next) = OpenAIMessageConverter::convert_messages_to_responses_input(vec![
            user,
            assistant,
            tool_result,
        ]);
        assert_eq!(next[1].get("content"), output[0].get("content"));
        let checkpoint = Checkpoint {
            id: "r1".into(),
            request: websocket_body(json!({"input":first})).unwrap(),
            output,
        };
        let next = websocket_body(json!({"input":next})).unwrap();
        let delta =
            incremental(&next, &checkpoint).expect("canonical replay should match provider output");
        assert_eq!(
            delta["input"],
            json!([{"type":"function_call_output","call_id":"call1","output":"file contents"}])
        );
        let mut changed_content = next.clone();
        changed_content["input"][1]["content"] = json!([{"type":"reasoning_text","text":"edited"}]);
        assert!(incremental(&changed_content, &checkpoint).is_none());
        let mut changed = next;
        changed["input"][2]["phase"] = json!("final_answer");
        assert!(incremental(&changed, &checkpoint).is_none());
    }
}

#[test]
fn empty_reasoning_content_is_equivalent_to_absent_but_other_content_is_not() {
    let absent = json!({"type":"reasoning","summary":[],"encrypted_content":"opaque"});
    let mut empty = absent.clone();
    empty["content"] = json!([]);
    assert_eq!(comparable(empty), comparable(absent.clone()));

    for content in [
        json!([{"type":"reasoning_text","text":"Preserve this"}]),
        json!(null),
        json!({}),
        json!(""),
    ] {
        let mut present = absent.clone();
        present["content"] = content;
        assert_ne!(comparable(present), comparable(absent.clone()));
    }
    for kind in ["message", "function_call", "future_item"] {
        let absent = json!({"type":kind});
        let mut empty = absent.clone();
        empty["content"] = json!([]);
        assert_ne!(comparable(empty), comparable(absent));
    }
}

#[test]
fn reasoning_state_changes_still_require_full_context() {
    let reasoning =
        json!({"type":"reasoning","content":[],"summary":[],"encrypted_content":"opaque"});
    let checkpoint = Checkpoint {
        id: "r1".into(),
        request: json!({"input":[]}),
        output: vec![reasoning.clone()],
    };
    let mut next = json!({"input":[reasoning,{"type":"function_call_output","call_id":"c1","output":"result"}]});
    next["input"][0].as_object_mut().unwrap().remove("content");
    assert_eq!(
        incremental(&next, &checkpoint).unwrap()["input"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for (field, value) in [
        (
            "content",
            json!([{"type":"reasoning_text","text":"new content"}]),
        ),
        (
            "summary",
            json!([{"type":"summary_text","text":"edited summary"}]),
        ),
        ("encrypted_content", json!("different-state")),
        ("unknown_field", json!(true)),
    ] {
        let mut changed = next.clone();
        changed["input"][0][field] = value;
        assert!(incremental(&changed, &checkpoint).is_none(), "{field}");
    }
}
