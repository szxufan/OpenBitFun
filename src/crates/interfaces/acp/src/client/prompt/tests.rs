use std::time::Duration;

use agent_client_protocol::schema::{CancelNotification, NewSessionResponse, StopReason};
use agent_client_protocol::{ActiveSession, Agent, ByteStreams, Client, Error, SessionMessage};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use super::super::transport::{handle_transport_closed, AcpTransport};
use super::{AcpPrompt, PromptError};

const DEADLINE: Duration = Duration::from_secs(2);

pub(in crate::client) struct Harness {
    stop_requested: watch::Sender<()>,
    pub(in crate::client) active: ActiveSession<'static, Agent>,
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    connection_task: JoinHandle<Result<(), Error>>,
}

impl Harness {
    pub(in crate::client) async fn new() -> Self {
        let (client, agent) = tokio::io::duplex(16 * 1024);
        let (client_reader, client_writer) = tokio::io::split(client);
        let (agent_reader, agent_writer) = tokio::io::split(agent);
        let transport = AcpTransport::new(ByteStreams::new(
            client_writer.compat_write(),
            client_reader.compat(),
        ));
        let (ready_tx, ready_rx) = oneshot::channel();
        let connection_task = tokio::spawn(async move {
            Client
                .builder()
                .on_receive_notification(
                    handle_transport_closed,
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_with(transport, async move |cx| {
                    let active =
                        cx.attach_session(NewSessionResponse::new("test-session"), vec![])?;
                    ready_tx.send(active).map_err(|_| Error::internal_error())?;
                    std::future::pending::<Result<(), Error>>().await
                })
                .await
        });
        let active = tokio::time::timeout(DEADLINE, ready_rx)
            .await
            .expect("connection should start")
            .expect("connection should publish a session");
        Self {
            stop_requested: watch::channel(()).0,
            active,
            reader: BufReader::new(agent_reader),
            writer: agent_writer,
            connection_task,
        }
    }

    pub(in crate::client) async fn request(&mut self) -> Value {
        let mut line = String::new();
        tokio::time::timeout(DEADLINE, self.reader.read_line(&mut line))
            .await
            .expect("request should reach agent")
            .expect("agent input should remain open");
        serde_json::from_str(&line).expect("valid ACP JSON-RPC request")
    }

    pub(in crate::client) async fn send(&mut self, message: Value) {
        let mut bytes = serde_json::to_vec(&message).unwrap();
        bytes.push(b'\n');
        self.writer.write_all(&bytes).await.unwrap();
    }

    fn prompt(&self, input: &str) -> AcpPrompt {
        AcpPrompt::start(
            &self.active,
            input.into(),
            None,
            None,
            self.stop_requested.subscribe(),
        )
    }

    pub(in crate::client) async fn reply(&mut self, method: &str, mut response: Value) -> Value {
        let request = self.request().await;
        assert_eq!(request["method"], method);
        response["jsonrpc"] = json!("2.0");
        response["id"] = request["id"].clone();
        self.send(response).await;
        request["params"].clone()
    }

    pub(in crate::client) async fn finish_with_text(
        &mut self,
        text: &str,
        stop_reason: &str,
    ) -> Value {
        let request = self.request().await;
        assert_eq!(request["method"], "session/prompt");
        self.send(json!({
            "jsonrpc": "2.0", "method": "session/update",
            "params": {"sessionId": request["params"]["sessionId"], "update": {
                "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}
            }}
        }))
        .await;
        self.send(json!({
            "jsonrpc": "2.0", "id": request["id"], "result": {"stopReason": stop_reason}
        }))
        .await;
        request["params"].clone()
    }

    async fn next(&mut self, prompt: &mut AcpPrompt) -> Result<SessionMessage, PromptError> {
        tokio::time::timeout(DEADLINE, prompt.read_update(&mut self.active))
            .await
            .expect("prompt must settle without relying on a user-supplied timeout")
    }
}

fn protocol_failure(error: PromptError) -> Error {
    match error {
        PromptError::Protocol(error) => error,
        other => panic!("expected an agent protocol error, got {other:?}"),
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.connection_task.abort();
    }
}

#[tokio::test]
async fn prompt_error_is_reported_and_the_same_session_can_retry() {
    let mut harness = Harness::new().await;
    let mut prompt = harness.prompt("offline request");
    let request = harness.request().await;
    assert_eq!(request["method"], "session/prompt");
    assert_eq!(request["params"]["sessionId"], "test-session");
    assert_eq!(request["params"]["prompt"][0]["text"], "offline request");
    harness
        .send(json!({
            "jsonrpc": "2.0",
            "id": request["id"],
            "error": {
                "code": -32603,
                "message": "Network connection failed",
                "data": { "code": "ENETUNREACH" }
            }
        }))
        .await;
    let error = protocol_failure(harness.next(&mut prompt).await.unwrap_err());
    assert_eq!(error.message, "Network connection failed");
    assert_eq!(error.data.unwrap()["code"], "ENETUNREACH");
    assert!(!harness.connection_task.is_finished());

    let mut retry = harness.prompt("retry after reconnect");
    let request = harness.request().await;
    harness
        .send(json!({
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": { "stopReason": "end_turn" }
        }))
        .await;
    assert!(matches!(
        harness.next(&mut retry).await.unwrap(),
        SessionMessage::StopReason(StopReason::EndTurn)
    ));
}

#[tokio::test]
async fn partial_output_is_preserved_before_a_prompt_error() {
    let mut harness = Harness::new().await;
    let mut prompt = harness.prompt("request");
    let request = harness.request().await;
    harness
        .send(json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "test-session",
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": "partial output" }
                }
            }
        }))
        .await;
    harness
        .send(json!({
            "jsonrpc": "2.0",
            "id": request["id"],
            "error": { "code": -32603, "message": "Connection reset" }
        }))
        .await;
    assert!(matches!(
        harness.next(&mut prompt).await.unwrap(),
        SessionMessage::SessionMessage(_)
    ));
    assert_eq!(
        protocol_failure(harness.next(&mut prompt).await.unwrap_err()).message,
        "Connection reset"
    );
}

#[tokio::test]
async fn cancellation_still_returns_the_agent_stop_reason() {
    let mut harness = Harness::new().await;
    let mut prompt = harness.prompt("request");
    let request = harness.request().await;
    harness
        .active
        .connection()
        .send_notification(CancelNotification::new(harness.active.session_id().clone()))
        .unwrap();
    assert_eq!(harness.request().await["method"], "session/cancel");
    harness
        .send(json!({
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": { "stopReason": "cancelled" }
        }))
        .await;
    assert!(matches!(
        harness.next(&mut prompt).await.unwrap(),
        SessionMessage::StopReason(StopReason::Cancelled)
    ));
}

#[tokio::test]
async fn agent_output_eof_fails_a_pending_prompt_even_if_its_input_is_open() {
    let mut harness = Harness::new().await;
    let mut prompt = harness.prompt("request");
    let _ = harness.request().await;
    harness.writer.shutdown().await.unwrap();
    assert!(harness.next(&mut prompt).await.is_err());
}

#[tokio::test]
async fn stopping_the_connection_wakes_a_pending_prompt() {
    let mut harness = Harness::new().await;
    let mut prompt = harness.prompt("request");
    let _ = harness.request().await;
    harness.connection_task.abort();
    assert!(harness.next(&mut prompt).await.is_err());
}

#[tokio::test]
async fn a_final_response_is_preserved_when_the_agent_immediately_closes_output() {
    let mut harness = Harness::new().await;
    let mut prompt = harness.prompt("request");
    let request = harness.request().await;
    harness
        .send(json!({
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": { "stopReason": "end_turn" }
        }))
        .await;
    harness.writer.shutdown().await.unwrap();
    assert!(matches!(
        harness.next(&mut prompt).await.unwrap(),
        SessionMessage::StopReason(StopReason::EndTurn)
    ));
}

#[tokio::test]
async fn an_error_response_is_preserved_when_the_agent_immediately_closes_output() {
    let mut harness = Harness::new().await;
    let mut prompt = harness.prompt("request");
    let request = harness.request().await;
    harness
        .send(json!({
            "jsonrpc": "2.0",
            "id": request["id"],
            "error": { "code": -32603, "message": "Network connection failed" }
        }))
        .await;
    harness.writer.shutdown().await.unwrap();
    assert_eq!(
        protocol_failure(harness.next(&mut prompt).await.unwrap_err()).message,
        "Network connection failed"
    );
}
