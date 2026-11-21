use crate::{OpenBitFunError, OpenBitFunResult};
use log::{debug, info};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Registration {
    pub id: u64,
    pub token: CancellationToken,
    pub client_id: Option<String>,
}

static SLOT: Mutex<Option<Registration>> = Mutex::const_new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// One pipeline at a time, including the publication of its final report.
pub static PIPELINE: Mutex<()> = Mutex::const_new(());

pub async fn run_until_cancelled<T>(
    token: &CancellationToken,
    work: impl Future<Output = OpenBitFunResult<T>>,
) -> OpenBitFunResult<T> {
    tokio::select! {
        biased;
        _ = token.cancelled() => Err(OpenBitFunError::service("Insights generation cancelled")),
        result = work => result,
    }
}

/// Registers a new insights generation task, cancelling any previous one.
pub async fn register(client_id: Option<String>) -> Registration {
    let token = CancellationToken::new();
    let registration = Registration {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        token,
        client_id,
    };
    let mut slot = SLOT.lock().await;
    if let Some(old) = slot.take() {
        old.token.cancel();
        debug!("Cancelled previous insights generation");
    }
    *slot = Some(registration.clone());
    registration
}

/// Cancels the current insights generation task.
pub async fn cancel(client_id: Option<&str>) -> Result<(), String> {
    let slot = SLOT.lock().await;
    let current = slot
        .as_ref()
        .filter(|current| client_id.is_none() || current.client_id.as_deref() == client_id)
        .ok_or_else(|| "No matching insights generation in progress".to_string())?;
    current.token.cancel();
    info!("Insights generation cancelled by user");
    Ok(())
}

/// Only the still-current, uncancelled generation may publish a report.
pub async fn claim_completion(id: u64) -> bool {
    let mut slot = SLOT.lock().await;
    if slot
        .as_ref()
        .is_some_and(|current| current.id == id && !current.token.is_cancelled())
    {
        *slot = None;
        true
    } else {
        false
    }
}

pub async fn unregister(id: u64) {
    let mut slot = SLOT.lock().await;
    if slot.as_ref().is_some_and(|current| current.id == id) {
        *slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelling_pipeline_drops_in_flight_work_without_waiting_for_a_model() {
        struct InFlight<'a>(&'a std::sync::atomic::AtomicUsize);
        impl Drop for InFlight<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }

        let token = CancellationToken::new();
        let active = std::sync::atomic::AtomicUsize::new(0);
        let calls = async {
            active.fetch_add(1, Ordering::SeqCst);
            let _request = InFlight(&active);
            std::future::pending::<OpenBitFunResult<()>>().await
        };
        let mut pipeline = Box::pin(run_until_cancelled(&token, calls));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(pipeline.as_mut().poll(&mut context).is_pending());
        assert_eq!(active.load(Ordering::SeqCst), 1);
        token.cancel();
        assert!(pipeline.await.is_err());
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn old_task_cannot_unregister_a_new_generation() {
        let old = register(Some("old".to_string())).await;
        let current = register(Some("current".to_string())).await;
        assert!(old.token.is_cancelled());

        unregister(old.id).await;
        assert!(cancel(Some("old")).await.is_err());
        assert!(!current.token.is_cancelled());
        assert!(!claim_completion(old.id).await);
        cancel(Some("current"))
            .await
            .expect("current generation remains registered");

        assert!(current.token.is_cancelled());
        assert!(!claim_completion(current.id).await);
        unregister(current.id).await;
    }
}
