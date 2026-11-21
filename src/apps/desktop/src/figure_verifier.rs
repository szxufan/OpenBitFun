//! The producing desktop shell checks a figure without navigating a webview.
use openbitfun_core::agentic::inline_figure::{register_figure_verifier, FigureCheckResult};
use std::{sync::Arc, time::Duration};
use tauri::{AppHandle, Emitter, EventTarget, Listener, Manager};

pub(crate) fn install(app: AppHandle) {
    register_figure_verifier(Arc::new(move |request| {
        let app = app.clone();
        Box::pin(async move {
            if app.get_webview("main").is_none() {
                return Err("FlowChat renderer unavailable on the producing device".into());
            }
            let id = uuid::Uuid::new_v4();
            let reply = format!("figure-check-result-{id}");
            let (send, receive) = tokio::sync::oneshot::channel();
            let handle = app.once(reply, move |event| {
                let result =
                    serde_json::from_str::<Result<FigureCheckResult, String>>(event.payload())
                        .map_err(|_| "Invalid renderer response".to_owned())
                        .and_then(|result| result);
                let _ = send.send(result);
            });
            let (progress, stages) = tokio::sync::watch::channel(());
            let stage_handle = app.listen(format!("figure-check-stage-{id}"), move |_| {
                let _ = progress.send(());
            });
            // Also unsubscribe if the tool future is cancelled or the shell reloads.
            struct Subscription(AppHandle, tauri::EventId);
            impl Drop for Subscription {
                fn drop(&mut self) {
                    self.0.unlisten(self.1);
                }
            }
            let _subscription = Subscription(app.clone(), handle);
            let _stages = Subscription(app.clone(), stage_handle);
            // Each projection and the final frame inspection have their own
            // deadline. Watch for a stalled stage, not the sum of healthy stages.
            let timeout_secs = if request.source.contains("data-visual-atlas") {
                75
            } else {
                30
            };
            app.emit_to(
                EventTarget::webview("main"),
                "figure-check",
                serde_json::json!({"id": id, "request": request}),
            )
            .map_err(|e| e.to_string())?;
            wait_for_renderer(receive, stages, Duration::from_secs(timeout_secs)).await
        })
    }));
}

async fn wait_for_renderer(
    mut receive: tokio::sync::oneshot::Receiver<Result<FigureCheckResult, String>>,
    mut stages: tokio::sync::watch::Receiver<()>,
    idle_timeout: Duration,
) -> Result<FigureCheckResult, String> {
    loop {
        tokio::select! {
            result = &mut receive => {
                return result.map_err(|_| "FlowChat renderer disconnected".to_owned())?;
            }
            change = stages.changed() => {
                change.map_err(|_| "FlowChat renderer disconnected".to_owned())?;
            }
            _ = tokio::time::sleep(idle_timeout) => {
                return Err("FlowChat renderer did not respond on the producing device".to_owned());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn forwards_the_final_diagnostic_after_two_slow_map_stages() {
        let (result, receive) = tokio::sync::oneshot::channel();
        let (progress, stages) = tokio::sync::watch::channel(());
        let renderer = tokio::spawn(async move {
            for _ in 0..2 {
                tokio::time::sleep(Duration::from_secs(59)).await;
                progress.send(()).unwrap();
            }
            tokio::time::sleep(Duration::from_secs(12)).await;
            let _ = result.send(Err("Missing stop coordinates".into()));
            progress
        });
        assert_eq!(
            wait_for_renderer(receive, stages, Duration::from_secs(75))
                .await
                .err()
                .as_deref(),
            Some("Missing stop coordinates")
        );
        renderer.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn still_times_out_when_the_renderer_stalls() {
        let (_result, receive) = tokio::sync::oneshot::channel();
        let (_progress, stages) = tokio::sync::watch::channel(());
        assert!(wait_for_renderer(receive, stages, Duration::from_secs(75))
            .await
            .err()
            .unwrap()
            .contains("did not respond"));
    }
}
