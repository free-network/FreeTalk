#![allow(dead_code)]

pub mod avatar;
pub mod ecies;
pub mod markdown;
pub mod message_actions;
pub mod messaging;

use ed25519_dalek::VerifyingKey;
use freenet_stdlib::prelude::{ContractCode, ContractKey, Parameters};
use river_core::board_state::member::MemberId;
use std::time::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(inline_js = "
export function get_current_time() {
    return Date.now();
}
export function format_time_local(timestamp_ms) {
    const date = new Date(timestamp_ms);
    return date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', hour12: false });
}
export function format_full_datetime_local(timestamp_ms) {
    const date = new Date(timestamp_ms);
    return date.toLocaleString(undefined, {
        weekday: 'short',
        year: 'numeric',
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
        second: '2-digit',
        hour12: false
    });
}
export function js_copy_to_clipboard(text) {
    // execCommand('copy') works in sandboxed iframes without allow-clipboard-write
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.style.position = 'fixed';
    ta.style.left = '-9999px';
    document.body.appendChild(ta);
    ta.select();
    document.execCommand('copy');
    document.body.removeChild(ta);
}
")]
extern "C" {
    fn get_current_time() -> f64;
    fn format_time_local(timestamp_ms: f64) -> String;
    fn format_full_datetime_local(timestamp_ms: f64) -> String;
    fn js_copy_to_clipboard(text: &str);
}

/// Copy text to clipboard. Works in sandboxed iframes where the Clipboard API is blocked.
pub fn copy_to_clipboard(text: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        js_copy_to_clipboard(text);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = text;
    }
}

pub fn get_current_system_time() -> SystemTime {
    #[cfg(target_arch = "wasm32")]
    {
        // Convert milliseconds since epoch to a Duration
        let millis = get_current_time();
        let duration_since_epoch = Duration::from_millis(millis as u64);
        UNIX_EPOCH + duration_since_epoch
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        SystemTime::now()
    }
}

/// Format a UTC timestamp as a local time string (HH:MM format)
pub fn format_utc_as_local_time(timestamp_ms: i64) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        format_time_local(timestamp_ms as f64)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        use chrono::{Local, TimeZone, Utc};
        let utc_time = Utc.timestamp_millis_opt(timestamp_ms).unwrap();
        utc_time.with_timezone(&Local).format("%H:%M").to_string()
    }
}

/// Format a UTC timestamp as a full local datetime string for tooltips
pub fn format_utc_as_full_datetime(timestamp_ms: i64) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        format_full_datetime_local(timestamp_ms as f64)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        use chrono::{Local, TimeZone, Utc};
        let utc_time = Utc.timestamp_millis_opt(timestamp_ms).unwrap();
        utc_time
            .with_timezone(&Local)
            .format("%a, %b %d, %Y %H:%M:%S")
            .to_string()
    }
}

// Helper function to create a Duration from seconds
pub fn seconds(s: u64) -> Duration {
    Duration::from_secs(s)
}

// Helper function to create a Duration from milliseconds
pub fn millis(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

/// A WASM-compatible sleep function that works in both browser and native environments
pub async fn sleep(duration: Duration) {
    #[cfg(target_arch = "wasm32")]
    {
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            let window = web_sys::window().unwrap();
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                &resolve,
                duration.as_millis() as i32,
            );
        });
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        // Use futures_timer for non-WASM environments to maintain compatibility
        let _ = futures_timer::Delay::new(duration).await;
    }
}

#[cfg(feature = "example-data")]
mod name_gen;
#[cfg(feature = "example-data")]
pub use name_gen::random_full_name;

use crate::constants::BOARD_CONTRACT_WASM;
use river_core::board_state::ChatBoardParametersV1;

pub fn to_cbor_vec<T: serde::Serialize>(value: &T) -> Vec<u8> {
    let mut buffer = Vec::new();
    ciborium::ser::into_writer(value, &mut buffer).unwrap();
    buffer
}

pub fn from_cbor_slice<T: serde::de::DeserializeOwned>(data: &[u8]) -> T {
    ciborium::de::from_reader(data).unwrap()
}

pub fn owner_vk_to_contract_key(owner_vk: &VerifyingKey) -> ContractKey {
    let params = ChatBoardParametersV1 { owner: *owner_vk };
    let params_bytes = to_cbor_vec(&params);
    let parameters = Parameters::from(params_bytes);
    let contract_code = ContractCode::from(BOARD_CONTRACT_WASM);
    // Use the full ContractKey constructor that includes the code hash
    ContractKey::from_params_and_code(parameters, &contract_code)
}

/// Defer a closure's execution via setTimeout(0), breaking out of the current WASM call stack.
///
/// IMPORTANT: Signal mutations (BOARDS.with_mut(), BOARDS.write(), CURRENT_BOARD.write(), etc.)
/// must always be wrapped in defer() when called from spawn_local tasks or synchronous event
/// handlers (onclick, etc.). This defers execution to a clean context where no Dioxus RefCell
/// borrows are active, preventing "RefCell already borrowed" panics in dioxus-core diff/node.rs.
///
/// # Example
/// ```ignore
/// onclick: move |_| {
///     crate::util::defer(move || {
///         BOARDS.write().map.remove(&key);
///     });
/// };
/// ```
#[cfg(target_arch = "wasm32")]
pub fn defer<F>(f: F)
where
    F: FnOnce() + 'static,
{
    use wasm_bindgen::prelude::*;
    let cb = Closure::once_into_js(f);
    web_sys::window()
        .expect("no window")
        .set_timeout_with_callback(&cb.into())
        .ok();
}

/// Non-WASM fallback: just run the closure directly
#[cfg(not(target_arch = "wasm32"))]
pub fn defer<F>(f: F)
where
    F: FnOnce() + 'static,
{
    f();
}

/// Spawn a future via setTimeout(0), breaking out of the current WASM call stack.
///
/// IMPORTANT: spawn_local runs within wasm-bindgen-futures' task scheduler,
/// which may hold a RefCell borrow when polling tasks. If the future reads/writes
/// signals that are currently borrowed, this causes a RefCell re-entrant panic
/// (especially on Firefox mobile).
///
/// This helper uses setTimeout(0) to schedule the spawn in a completely clean
/// execution context, preventing such panics.
#[cfg(target_arch = "wasm32")]
pub fn safe_spawn_local<F>(f: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::spawn_local;

    let boxed: std::pin::Pin<Box<dyn std::future::Future<Output = ()>>> = Box::pin(f);
    let cb = Closure::once_into_js(move || {
        spawn_local(boxed);
    });
    web_sys::window()
        .expect("no window")
        .set_timeout_with_callback(&cb.into())
        .ok();
}

/// Non-WASM fallback: just run the future with spawn_local directly
#[cfg(not(target_arch = "wasm32"))]
pub fn safe_spawn_local<F>(_f: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    // In non-WASM we don't have a task scheduler, so this is a no-op
    // (Could use tokio::spawn in a real server context)
}

/// Generate a consistent HSL color string from a MemberId.
/// Uses the hash value to determine hue, with fixed saturation and lightness
/// for good visibility on dark backgrounds.
pub fn member_id_to_color(member_id: &MemberId) -> String {
    // Use the hash value to generate a hue (0-360)
    let hash = member_id.0 .0;
    let hue = ((hash.abs() as u64) % 360) as u16;
    // Use moderate saturation and lightness for visibility
    format!("hsl({}, 65%, 55%)", hue)
}
