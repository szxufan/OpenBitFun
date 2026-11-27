//! Keep Tauri browser children out of the main WebView's vertical GTK layout.

use gtk::prelude::*;
use gtk::{OverlaySignals, Widget};
use tauri::{LogicalPosition, LogicalSize, Webview, WebviewWindow, Window};
use tokio::sync::oneshot;

const OVERLAY_NAME: &str = "openbitfun-browser-overlay";
const BOUNDS_KEY: &str = "openbitfun-browser-overlay-bounds";

pub(crate) fn install(window: &WebviewWindow) -> tauri::Result<()> {
    window.with_webview(|native| {
        let main = native.inner();
        let Some(parent) = main.parent().and_downcast::<gtk::Box>() else {
            log::error!("Main WebView is missing its GTK container");
            return;
        };

        let overlay = gtk::Overlay::new();
        overlay.set_widget_name(OVERLAY_NAME);
        overlay.connect_get_child_position(|_, child| {
            // SAFETY: This module exclusively uses this key for a Rectangle.
            // GTK calls this on its main thread and we copy the value immediately.
            unsafe {
                child
                    .data::<gtk::gdk::Rectangle>(BOUNDS_KEY)
                    .map(|value| *value.as_ref())
            }
        });

        // Only the primary child contributes to the window's size request.
        parent.remove(&main);
        overlay.add(&main);
        parent.pack_start(&overlay, true, true, 0);
        overlay.show();
    })
}

pub(crate) async fn create_child(
    window: Window,
    builder: tauri::webview::WebviewBuilder<tauri::Wry>,
    position: LogicalPosition<f64>,
    size: LogicalSize<f64>,
) -> Result<Webview, String> {
    let (sender, receiver) = oneshot::channel();
    let host = window.clone();
    window
        .run_on_main_thread(move || {
            // Attach and hide in the same GTK iteration as Tauri creates the
            // child so the vertical box cannot lay it out or paint it first.
            let webview = host.add_child(builder.focused(false), position, size);
            let result = webview
                .map_err(|error| format!("failed to create browser webview: {error}"))
                .and_then(|webview| {
                    let (attached, mut attachment) = oneshot::channel();
                    // Tauri dispatches with_webview inline on its main thread.
                    let result = webview
                        .with_webview(move |native| {
                            let result = attach_child(&native.inner().upcast(), position, size);
                            let _ = attached.send(result);
                        })
                        .map_err(|error| format!("failed to access browser webview: {error}"))
                        .and_then(|()| {
                            attachment.try_recv().map_err(|_| {
                                "browser webview GTK attachment did not run".to_string()
                            })?
                        });
                    if let Err(error) = result {
                        let _ = webview.close();
                        return Err(error);
                    }
                    Ok(webview)
                });
            let _ = sender.send(result);
        })
        .map_err(|error| format!("failed to schedule browser webview creation: {error}"))?;
    receiver
        .await
        .map_err(|_| "browser webview creation was canceled".to_string())?
}

fn attach_child(
    child: &Widget,
    position: LogicalPosition<f64>,
    size: LogicalSize<f64>,
) -> Result<(), String> {
    child.hide();
    let parent = child
        .parent()
        .and_downcast::<gtk::Box>()
        .ok_or_else(|| "browser webview is missing its GTK container".to_string())?;
    let overlay = parent
        .children()
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::Overlay>().ok())
        .find(|overlay| overlay.widget_name() == OVERLAY_NAME)
        .ok_or_else(|| "browser webview is missing its GTK overlay".to_string())?;

    save_bounds(child, position, size);
    parent.remove(child);
    overlay.add_overlay(child);
    Ok(())
}

pub(crate) async fn set_bounds(
    webview: &Webview,
    position: LogicalPosition<f64>,
    size: LogicalSize<f64>,
) -> Result<(), String> {
    let (sender, receiver) = oneshot::channel();
    webview
        .with_webview(move |native| {
            let child: Widget = native.inner().upcast();
            let result = child
                .parent()
                .and_downcast::<gtk::Overlay>()
                .ok_or_else(|| "browser webview is missing its GTK overlay".to_string())
                .map(|overlay| {
                    save_bounds(&child, position, size);
                    overlay.queue_allocate();
                });
            let _ = sender.send(result);
        })
        .map_err(|error| format!("set bounds failed: {error}"))?;
    receiver
        .await
        .map_err(|_| "browser webview bounds update was canceled".to_string())?
}

fn save_bounds(child: &Widget, position: LogicalPosition<f64>, size: LogicalSize<f64>) {
    let position = position.cast::<i32>();
    let size = size.cast::<i32>();
    let bounds = gtk::gdk::Rectangle::new(position.x, position.y, size.width, size.height);
    // SAFETY: Only this module accesses this key, always with a Rectangle.
    unsafe { child.set_data(BOUNDS_KEY, bounds) }
}
