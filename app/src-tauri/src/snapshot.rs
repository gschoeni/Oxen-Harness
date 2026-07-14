//! Native WKWebView snapshot (macOS): how `preview_screenshot` sees the page.
//!
//! The preview webview shows external localhost content, so there's no
//! JavaScript path to pixels. WKWebView's `takeSnapshot` renders the page
//! (including GPU-composited content) to an `NSImage` off the window's own
//! compositor — no screen-recording permission involved. The image is
//! converted to PNG via `NSBitmapImageRep`.

use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage};
use objc2_foundation::{NSDictionary, NSError};
use objc2_web_kit::WKWebView;

/// How long to wait for WebKit's snapshot callback before giving up — a hung
/// WebContent process must not hang the agent's whole turn.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);

/// Capture `webview`'s current page as PNG bytes.
pub(crate) async fn take_png(webview: tauri::webview::Webview) -> Result<Vec<u8>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<Vec<u8>, String>>();
    // The completion block is `Fn`, so hand the one-shot sender through a slot.
    let slot = std::sync::Mutex::new(Some(tx));

    webview
        .with_webview(move |platform| {
            // SAFETY: on macOS `PlatformWebview::inner()` hands over an *owned*
            // (+1 retained) pointer to the WKWebView — Tauri does
            // `Retained::into_raw` and never releases it. Reclaim it here so
            // repeated screenshots don't leak a webview (and its WebContent
            // process) each time. The pointer is valid and non-null; this
            // closure runs on the main thread (Tauri dispatches it through the
            // event loop), which is where WKWebView must be touched.
            let wk: Retained<WKWebView> =
                unsafe { Retained::from_raw(platform.inner().cast::<WKWebView>()) }
                    .expect("tauri hands over a non-null WKWebView");
            let block = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
                let result = png_from(image, error);
                if let Some(tx) = slot.lock().unwrap().take() {
                    let _ = tx.send(result);
                }
            });
            unsafe { wk.takeSnapshotWithConfiguration_completionHandler(None, &block) };
        })
        .map_err(|e| format!("snapshot: {e}"))?;

    match tokio::time::timeout(SNAPSHOT_TIMEOUT, rx).await {
        Ok(Ok(result)) => result,
        // The webview went away before the closure ran (sender dropped).
        Ok(Err(_)) => Err("the preview closed before the screenshot completed".to_string()),
        Err(_) => Err("the preview did not respond to the screenshot request \
                       (the page may be hung) — try reloading it"
            .to_string()),
    }
}

/// Convert the snapshot callback's `NSImage` into PNG bytes.
fn png_from(image: *mut NSImage, error: *mut NSError) -> Result<Vec<u8>, String> {
    if image.is_null() {
        // SAFETY: WebKit hands either an image or an error; both are valid
        // (or null) for the duration of the callback.
        let message = unsafe { error.as_ref() }
            .map(|e| e.localizedDescription().to_string())
            .unwrap_or_else(|| "no image produced".to_string());
        return Err(format!("snapshot failed: {message}"));
    }
    // SAFETY: non-null NSImage from the callback, used within its lifetime.
    let image = unsafe { &*image };
    let tiff = image.TIFFRepresentation().ok_or("snapshot has no bitmap data")?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff).ok_or("could not decode snapshot")?;
    let png: Retained<_> = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .ok_or("could not encode PNG")?;
    Ok(png.to_vec())
}
