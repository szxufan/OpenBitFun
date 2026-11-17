//! Responses connections and checkpoints partitioned by live conversation scope.
use super::{
    request_capacity::RequestCapacity, sse::ManagedResponseStream, AIClient, StreamResponse,
};
use crate::providers::{openai::common, shared};
use crate::trace::{ModelExchangeRequestAttempt, ModelExchangeTraceConfig};
use anyhow::Result;
use futures::{FutureExt, SinkExt, StreamExt};
use openbitfun_core_types::responses::{ResponsesContextMode, ResponsesTransport};
use openbitfun_core_types::{
    errors::{AiProviderError, ErrorCategory},
    ModelRequestContext,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

use tokio_tungstenite::{
    tungstenite::{self, protocol::Role, Message},
    WebSocketStream,
};
use tokio_util::sync::CancellationToken;

type Socket = WebSocketStream<reqwest::Upgraded>;
const IDLE_RETENTION: Duration = Duration::from_secs(60);

#[derive(Default, Debug)]
pub(crate) struct Sessions(Mutex<HashMap<String, Entry>>);

#[derive(Debug)]
struct Entry {
    owner: Weak<String>,
    state: Arc<AsyncMutex<Session>>,
}

#[derive(Default, Debug)]
struct Session {
    socket: Option<Socket>,
    checkpoint: Option<Checkpoint>,
    last_used: Option<Instant>,
}

#[derive(Debug)]
struct Checkpoint {
    id: String,
    request: Value,
    output: Vec<Value>,
}

impl Sessions {
    fn session(
        self: &Arc<Self>,
        scope: Option<&Arc<String>>,
        channel: &str,
    ) -> Arc<AsyncMutex<Session>> {
        let Some(scope) = scope else {
            return Arc::default();
        };
        let key = format!("{}:{channel}", scope.as_str());
        let mut entries = self.0.lock().unwrap();
        if let Some(entry) = entries.get(&key) {
            return entry.state.clone();
        }
        let state = Arc::new(AsyncMutex::new(Session::default()));
        entries.insert(
            key.clone(),
            Entry {
                owner: Arc::downgrade(scope),
                state: state.clone(),
            },
        );
        let cache = Arc::downgrade(self);

        // Expire host-owned resources independently of UI/controller connections.
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(IDLE_RETENTION).await;
                let Some(cache) = cache.upgrade() else { break };
                let mut entries = cache.0.lock().unwrap();
                let expired = entries.get(&key).is_none_or(|entry| {
                    if entry.owner.strong_count() == 0 {
                        return true;
                    }
                    if Arc::strong_count(&entry.state) == 1 {
                        if let Ok(state) = entry.state.try_lock() {
                            return state
                                .last_used
                                .is_none_or(|last| last.elapsed() >= IDLE_RETENTION);
                        }
                    }
                    false
                });
                if expired {
                    entries.remove(&key);
                    break;
                }
            }
        });
        state
    }
}

fn invalid(message: &str) -> anyhow::Error {
    AiProviderError::classified(message.into(), ErrorCategory::InvalidRequest).into()
}

pub(crate) fn validate(client: &AIClient) -> Result<()> {
    let settings = client.config.responses.unwrap_or_default();
    if settings.context == ResponsesContextMode::Incremental
        && settings.transport != ResponsesTransport::Websocket
    {
        return Err(invalid(
            "Responses incremental context requires WebSocket transport; HTTP uses full context",
        ));
    }
    if settings != Default::default()
        && !matches!(client.config.format.as_str(), "response" | "responses")
    {
        return Err(invalid(
            "Responses transport settings require the Responses API format",
        ));
    }
    Ok(())
}

fn websocket_body(mut body: Value) -> Result<Value> {
    let fields = body
        .as_object_mut()
        .ok_or_else(|| invalid("Responses body must be an object"))?;
    if fields.contains_key("previous_response_id")
        || fields.contains_key("conversation")
        || fields.contains_key("stream_id")
    {
        return Err(invalid(
            "Responses continuation fields are owned by the runtime and cannot be overridden",
        ));
    }
    if fields
        .get("store")
        .is_some_and(|value| value != &Value::Bool(false))
    {
        return Err(invalid("Responses WebSocket requests require store:false"));
    }
    if fields
        .get("background")
        .is_some_and(|value| value != &Value::Bool(false))
    {
        return Err(invalid(
            "Responses WebSocket transport does not support background requests",
        ));
    }
    fields
        .entry("include")
        .or_insert_with(|| serde_json::json!(["reasoning.encrypted_content"]));
    fields.remove("stream");
    fields.remove("background");
    fields.insert("type".into(), Value::String("response.create".into()));
    fields.insert("store".into(), Value::Bool(false));
    Ok(body)
}

/// Normalize only metadata and empty placeholders, preserving model-visible content.
fn comparable(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        match object.get("type").and_then(Value::as_str) {
            Some("message" | "function_call" | "reasoning") => {
                object.remove("id");
                object.remove("status");
            }
            _ => {}
        }
        // Providers can return reasoning.content: [] while canonical replay
        // omits it. Normalize only that empty placeholder, never nonempty or
        // unknown content, and leave both the checkpoint and wire input intact.
        if object.get("type").and_then(Value::as_str) == Some("reasoning")
            && object
                .get("content")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        {
            object.remove("content");
        }
        if object.get("type").and_then(Value::as_str) == Some("function_call") {
            if let Some(arguments) = object
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
            {
                object.insert("arguments".into(), arguments);
            }
        }
        if let Some(parts) = object.get_mut("content").and_then(Value::as_array_mut) {
            for part in parts {
                if part.get("type").and_then(Value::as_str) == Some("output_text") {
                    if let Some(part) = part.as_object_mut() {
                        part.remove("annotations");
                        part.remove("logprobs");
                    }
                }
            }
        }
    }
    value
}

fn incremental(full: &Value, previous: &Checkpoint) -> Option<Value> {
    let mut old = previous.request.clone();
    let mut current = full.clone();
    let old_input = old.as_object_mut()?.remove("input")?;
    let input = current.as_object_mut()?.remove("input")?;
    if old != current {
        return None;
    }
    let input = input.as_array()?;
    let baseline = old_input.as_array()?.iter().chain(&previous.output);
    let count = baseline.clone().count();
    if input.len() < count
        || !baseline
            .zip(input)
            .all(|(a, b)| comparable(a.clone()) == comparable(b.clone()))
    {
        return None;
    }
    current["input"] = Value::Array(input[count..].to_vec());
    current["previous_response_id"] = Value::String(previous.id.clone());
    Some(current)
}

async fn authorize(client: &AIClient) -> Result<()> {
    if let Some(auth) = &client.request_authorization {
        auth.validate().await?;
    }
    #[cfg(feature = "subscription-auth")]
    if let Some(identity) = &client.account_identity {
        identity.validate(client).await?;
    }
    Ok(())
}

fn request(client: &AIClient, context: Option<&ModelRequestContext>) -> reqwest::RequestBuilder {
    let url = &client.config.request_url;
    shared::apply_affinity_headers(
        client,
        common::apply_headers(client, client.websocket_client.get(url)),
        url,
        context,
    )
}

async fn connect(
    client: &AIClient,
    context: Option<&ModelRequestContext>,
    permit: &super::request_capacity::RequestPermit,
) -> Result<Socket> {
    let key = tungstenite::handshake::client::generate_key();
    let mut handshake = request(client, context)
        .version(reqwest::Version::HTTP_11)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", &key)
        .build()?;
    handshake.headers_mut().entry("openai-beta").or_insert(
        reqwest::header::HeaderValue::from_static("responses_websockets=2026-02-06"),
    );
    let response = client
        .websocket_client
        .execute(handshake)
        .await
        .map_err(|error| super::sse::transport_provider_error("Responses WebSocket", &error))?;
    if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.text().await?;
        if super::sse::is_provider_concurrency_limit(status, &body) {
            permit.concurrency_rejected(Duration::from_millis(
                super::sse::provider_recovery_hint_ms(&headers, &body).unwrap_or(2000),
            ));
        }
        return Err(super::sse::http_provider_error(
            "Responses WebSocket",
            status,
            &body,
            "handshake rejected",
            super::sse::provider_recovery_hint_ms(&headers, &body),
        )
        .into());
    }
    let expected = tungstenite::handshake::derive_accept_key(key.as_bytes());
    if response
        .headers()
        .get("sec-websocket-accept")
        .and_then(|v| v.to_str().ok())
        != Some(expected.as_str())
        || !response
            .headers()
            .get("connection")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
            })
        || !response
            .headers()
            .get("upgrade")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
    {
        return Err(invalid("Invalid Responses WebSocket upgrade response"));
    }
    Ok(WebSocketStream::from_raw_socket(response.upgrade().await?, Role::Client, None).await)
}

pub(crate) async fn send(
    client: &AIClient,
    body: Value,
    context: Option<ModelRequestContext>,
    trace: Option<ModelExchangeTraceConfig>,
) -> Result<StreamResponse> {
    validate(client)?;
    let full = websocket_body(body)?;
    use sha2::{Digest, Sha256};
    let channel = format!("{:x}", Sha256::digest(serde_json::to_vec(&client.config)?));
    let state = client.responses_sessions.session(
        context.as_ref().and_then(|c| c.conversation_scope.as_ref()),
        &channel,
    );
    let mut state = state.lock_owned().await;
    let capacity = RequestCapacity::for_request(&request(client, context.as_ref()))?;
    let mut permit = capacity.acquire().await;
    authorize(client).await?;
    let start = Instant::now();
    // Taking the socket before any await ensures cancellation never leaves an in-flight socket reusable.
    let socket = state.socket.take();
    let previous = state.checkpoint.take();
    let mut socket = match socket {
        Some(mut socket) => {
            let usable = loop {
                match socket.next().now_or_never() {
                    None => break true,
                    Some(Some(Ok(Message::Ping(_)))) => {
                        if !matches!(
                            tokio::time::timeout(Duration::from_secs(1), socket.flush()).await,
                            Ok(Ok(()))
                        ) {
                            break false;
                        }
                    }
                    Some(Some(Ok(Message::Pong(_)))) => {}
                    _ => break false,
                }
            };
            if usable {
                Some(socket)
            } else {
                None
            }
        }
        None => None,
    };
    let reused = socket.is_some();
    let payload = if reused
        && client.config.responses.unwrap_or_default().context == ResponsesContextMode::Incremental
    {
        previous
            .as_ref()
            .and_then(|previous| incremental(&full, previous))
            .unwrap_or_else(|| full.clone())
    } else {
        full.clone()
    };
    let mut payload = payload;
    let mut attempt = 0;
    // Inspect only the first frame for an explicit rejection. Once the provider
    // acknowledges a response, neither this transport nor the runtime may replay it.
    let (first_event, trace_handle) = loop {
        attempt += 1;
        authorize(client).await?;
        let trace_handle = if let Some(trace) = &trace {
            trace
                .sink
                .request_attempt_started(&ModelExchangeRequestAttempt {
                    request_url: client.config.request_url.clone(),
                    request_body: trace.capture_request_body.then(|| payload.clone()),
                    attempt_number: attempt,
                    round_attempt: trace.round_attempt().cloned(),
                })
                .await
        } else {
            None
        };
        // Handshake failures are known to precede response.create and remain retryable.
        let connected = if socket.is_none() {
            let limit = client
                .stream_options
                .ttft_timeout
                .unwrap_or(Duration::from_secs(30))
                .saturating_sub(start.elapsed())
                .min(Duration::from_secs(AIClient::STREAM_CONNECT_TIMEOUT_SECS));
            match tokio::time::timeout(limit, connect(client, context.as_ref(), &permit)).await {
                Ok(Ok(connected)) => {
                    socket = Some(connected);
                    Ok(())
                }
                Ok(Err(error)) => Err(error),
                Err(_) => Err(AiProviderError::classified(
                    "Responses WebSocket connection timed out".into(),
                    ErrorCategory::Timeout,
                )
                .into()),
            }
        } else {
            Ok(())
        };
        let first = match connected {
            Err(error) => Err(error),
            Ok(()) => {
                let exchange = async {
                    let socket = socket.as_mut().unwrap();
                    // Revalidate after the network handshake as well as after admission.
                    authorize(client).await?;
                    socket
                        .send(Message::Text(payload.to_string().into()))
                        .await
                        .map_err(uncertain)?;
                    receive(socket).await
                };
                match client.stream_options.ttft_timeout {
                    Some(limit) => {
                        tokio::time::timeout(limit.saturating_sub(start.elapsed()), exchange)
                            .await
                            .map_err(uncertain)
                            .and_then(|r| r)
                    }
                    None => exchange.await,
                }
            }
        };
        let first = match first {
            Ok(first) => first,
            Err(error) => {
                if let Some(trace) = &trace {
                    trace
                        .sink
                        .request_attempt_failed(trace_handle.as_ref(), &error.to_string())
                        .await;
                }
                return Err(error);
            }
        };
        let event: Value = serde_json::from_str(&first).unwrap_or(Value::Null);
        let code = event.pointer("/error/code").and_then(Value::as_str);
        let rejected = event.get("type").and_then(Value::as_str) == Some("error");
        let missing = code == Some("previous_response_not_found")
            && payload.get("previous_response_id").is_some();
        let expired = code == Some("websocket_connection_limit_reached");
        if attempt == 1 && rejected && (missing || expired) {
            if let Some(trace) = &trace {
                trace
                    .sink
                    .request_attempt_failed(trace_handle.as_ref(), code.unwrap())
                    .await;
            }
            log::debug!("Responses WebSocket request rejected before acceptance; rebuilding full context: {}", code.unwrap());
            if expired {
                socket = None;
            }
            payload = full.clone();
            continue;
        }
        break (first, trace_handle);
    };
    log::debug!(
        "Responses WebSocket request: reused={}, incremental={}, attempt={}",
        reused,
        payload.get("previous_response_id").is_some(),
        attempt
    );
    let socket = socket.unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    let (tx_raw, rx_raw) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let completion = permit.take_completion();
    let client = client.clone();
    let timeout = client
        .stream_options
        .ttft_timeout
        .map(|limit| limit.saturating_sub(start.elapsed()));
    let expected_prompt_cache_key_hash =
        crate::providers::openai::responses::prompt_cache_key_hash(&full);
    let task = tokio::spawn(async move {
        let _permit = permit;
        let work = async {
            let mut wire = WireEvents {
                socket,
                full,
                pending: Some(first_event),
                terminal: false,
                checkpoint: None,
                output: std::collections::BTreeMap::new(),
            };
            let events = futures::stream::unfold(&mut wire, |wire| async {
                wire.next().await.map(|event| (event, wire))
            });
            let successful = crate::stream::handle_responses_events(
                Box::pin(events),
                tx,
                Some(tx_raw),
                timeout,
                client.stream_options.idle_timeout,
                expected_prompt_cache_key_hash,
                true,
            )
            .await;
            if successful && wire.checkpoint.is_some() {
                state.checkpoint = wire.checkpoint;
                state.socket = Some(wire.socket);
                state.last_used = Some(Instant::now());
            }
        };
        tokio::select! { _ = task_cancel.cancelled() => {}, _ = work => {} }
    });
    Ok(StreamResponse {
        stream: Box::pin(ManagedResponseStream::new(
            rx,
            cancel,
            task,
            Some(completion),
        )),
        raw_sse_rx: Some(rx_raw),
        trace_handle,
    })
}

fn uncertain(error: impl std::fmt::Display) -> anyhow::Error {
    let mut error = AiProviderError::classified(
        format!("Responses WebSocket delivery is uncertain; the request was not replayed: {error}"),
        ErrorCategory::Network,
    );
    error.delivery_uncertain = true;
    error.into()
}

async fn receive(socket: &mut Socket) -> Result<String> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => return Ok(text.to_string()),
            Some(Ok(Message::Ping(_))) => socket.flush().await.map_err(uncertain)?,
            Some(Ok(Message::Pong(_))) => {}
            Some(Err(error)) => return Err(uncertain(error)),
            _ => return Err(uncertain("connection closed before response completion")),
        }
    }
}

struct WireEvents {
    socket: Socket,
    full: Value,
    pending: Option<String>,
    terminal: bool,
    checkpoint: Option<Checkpoint>,
    output: std::collections::BTreeMap<usize, Value>,
}

impl WireEvents {
    async fn next(&mut self) -> Option<Result<String>> {
        if self.terminal {
            return None;
        }
        let text = match self.pending.take() {
            Some(text) => text,
            None => match receive(&mut self.socket).await {
                Ok(text) => text,
                Err(error) => return Some(Err(error)),
            },
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => return Some(Err(uncertain(error))),
        };
        if value.get("type").and_then(Value::as_str) == Some("response.output_item.done") {
            if let (Some(index), Some(item)) = (
                value.get("output_index").and_then(Value::as_u64),
                value.get("item"),
            ) {
                self.output.insert(index as usize, item.clone());
            }
        }
        match value.get("type").and_then(Value::as_str) {
            Some("response.completed" | "response.done") => {
                self.terminal = true;
                if let Some(id) = value
                    .pointer("/response/id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    let output = value
                        .pointer("/response/output")
                        .and_then(Value::as_array)
                        .cloned()
                        .or_else(|| {
                            (!self.output.is_empty())
                                .then(|| self.output.values().cloned().collect())
                        });
                    if let Some(output) = output {
                        self.checkpoint = Some(Checkpoint {
                            id: id.into(),
                            request: self.full.clone(),
                            output,
                        });
                    }
                }
            }
            Some("error" | "response.failed" | "response.incomplete") => self.terminal = true,
            _ => {}
        }
        Some(Ok(text))
    }
}

#[cfg(test)]
mod tests;
