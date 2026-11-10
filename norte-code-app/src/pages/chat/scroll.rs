//! One part of the conversation page.

#[allow(unused_imports)]
#[allow(clippy::wildcard_imports)]
use super::*;

/// Waits, on the wasm timer or a blocking thread natively.
#[cfg_attr(not(target_arch = "wasm32"), allow(clippy::unused_async))]
pub(super) async fn sleep(duration: std::time::Duration) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            let promise = js_sys::Promise::new(&mut |resolve, _reject| {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                    &resolve,
                    i32::try_from(duration.as_millis()).unwrap_or(i32::MAX),
                );
            });
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::sleep(duration);
    }
}

/// Waits one frame so the DOM has the appended text before scrolling.
#[cfg_attr(not(target_arch = "wasm32"), allow(clippy::unused_async))]
pub(super) async fn gloo_timeout() {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        if let Some(window) = web_sys::window() {
            let promise = js_sys::Promise::new(&mut |resolve, _reject| {
                let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
            });
            let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Native runs render synchronously enough; nothing to wait for.
    }
}

/// Whether the transcript is scrolled to (or near) its bottom.
pub(super) fn at_transcript_bottom() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast as _;
        if let Some(document) = web_sys::window().and_then(|window| window.document())
            && let Some(element) = document.get_element_by_id("transcript")
            && let Ok(element) = element.dyn_into::<web_sys::HtmlElement>()
        {
            let distance = element.scroll_height() - element.scroll_top() - element.client_height();
            // A delta can add more than a line between two frames; a tight
            // threshold turned following off mid-answer.
            return distance < 160;
        }
        true
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        true
    }
}

pub(super) fn scroll_transcript_to_end() {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast as _;
        if let Some(document) = web_sys::window().and_then(|window| window.document())
            && let Some(element) = document.get_element_by_id("transcript")
            && let Ok(element) = element.dyn_into::<web_sys::HtmlElement>()
        {
            element.set_scroll_top(element.scroll_height());
        }
    }
}
