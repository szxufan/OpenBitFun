use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use agent_client_protocol::schema::{CancelNotification, PromptRequest, PromptResponse};
use agent_client_protocol::{ActiveSession, Agent, ConnectionTo, Error, SessionMessage};
use futures::FutureExt;
use openbitfun_core::util::errors::{OpenBitFunError, OpenBitFunResult};
use serde_json::Value;
use tokio::sync::watch;

use super::manager::protocol_error;

const PROMPT_CANCEL_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub(super) struct AcpCancelHandle {
    pub(super) session_id: String,
    pub(super) connection: ConnectionTo<Agent>,
    stop_requested: watch::Sender<()>,
}

impl AcpCancelHandle {
    pub(super) fn new(
        active: &ActiveSession<'_, Agent>,
        stop_requested: watch::Sender<()>,
    ) -> Self {
        Self {
            session_id: active.session_id().to_string(),
            connection: active.connection(),
            stop_requested,
        }
    }

    pub(super) fn cancel(&self) -> OpenBitFunResult<()> {
        let sent = self
            .connection
            .send_notification(CancelNotification::new(self.session_id.clone()));
        // The prompt owns recovery even if writing the notification failed.
        if self.stop_requested.send(()).is_err() {
            sent.map_err(protocol_error)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(super) enum PromptError {
    Protocol(Error),
    OutcomeUnknown(&'static str),
}

impl From<PromptError> for OpenBitFunError {
    fn from(error: PromptError) -> Self {
        match error {
            PromptError::Protocol(error) => protocol_error(error),
            PromptError::OutcomeUnknown(reason) => Self::OutcomeUnknown(reason.into()),
        }
    }
}

/// Own the prompt response separately from the session notification queue.
/// ACP 0.12's ActiveSession::send_prompt only queues successful stop reasons;
/// its error callback terminates the connection without waking read_update.
pub(super) struct AcpPrompt {
    response: Pin<Box<dyn Future<Output = Result<PromptResponse, Error>> + Send>>,
    cancel_deadline: Pin<Box<dyn Future<Output = ()> + Send>>,
    deadline: Option<tokio::time::Instant>,
}

impl AcpPrompt {
    pub(super) fn start(
        active: &ActiveSession<'_, Agent>,
        prompt: String,
        message_id: Option<String>,
        deadline: Option<tokio::time::Instant>,
        mut stop_requested: watch::Receiver<()>,
    ) -> Self {
        let mut request = PromptRequest::new(active.session_id().clone(), vec![prompt.into()]);
        request.message_id = message_id;
        let response = active.connection().send_request(request).block_task();
        Self {
            deadline,
            response: Box::pin(response),
            cancel_deadline: Box::pin(async move {
                if stop_requested.changed().await.is_ok() {
                    tokio::time::sleep(PROMPT_CANCEL_GRACE).await;
                } else {
                    std::future::pending::<()>().await;
                }
            }),
        }
    }

    pub(super) async fn read_update(
        &mut self,
        active: &mut ActiveSession<'_, Agent>,
    ) -> Result<SessionMessage, PromptError> {
        let timeout = async {
            if let Some(deadline) = self.deadline {
                tokio::time::sleep_until(deadline).await;
            } else {
                std::future::pending::<()>().await;
            }
        };
        let response = tokio::select! {
            biased;
            () = &mut self.cancel_deadline => {
                // Prefer a real terminal result if it arrived at the deadline.
                self.response.as_mut().now_or_never().ok_or_else(|| PromptError::OutcomeUnknown(
                    "The ACP agent did not confirm the stop. Check the previous turn's work before sending it again."
                ))?
            }
            // Deliver queued partial output before the terminal response.
            update = active.read_update() => match update {
                Ok(update) => return Ok(update),
                Err(_) => self.response.as_mut().now_or_never().ok_or_else(|| PromptError::OutcomeUnknown(
                    "The ACP connection closed before the agent confirmed the turn outcome."
                ))?,
            },
            response = &mut self.response => response,
            () = timeout => return Err(PromptError::OutcomeUnknown(
                "The ACP prompt timed out before the agent confirmed the turn outcome."
            )),
        }.map_err(|error| {
            // The SDK distinguishes a dropped response channel from an agent's JSON-RPC error.
            if error.data.as_ref().and_then(Value::as_str).is_some_and(|detail| detail.starts_with("response to `session/prompt` never received:")) {
                PromptError::OutcomeUnknown("The ACP connection closed before the agent confirmed the turn outcome.")
            } else {
                PromptError::Protocol(error)
            }
        })?;
        Ok(SessionMessage::StopReason(response.stop_reason))
    }
}

#[cfg(test)]
#[path = "prompt/tests.rs"]
pub(super) mod tests;
