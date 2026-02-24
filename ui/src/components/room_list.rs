pub(crate) mod create_room_modal;
pub(crate) mod edit_room_modal;
pub(crate) mod receive_invitation_modal;
pub(crate) mod room_name_field;

use crate::components::app::chat_delegate::save_rooms_to_delegate;
use crate::components::app::document_title::mark_current_room_as_read;
use crate::components::app::{CREATE_ROOM_MODAL, CURRENT_ROOM, ROOMS};
use crate::util::ecies::unseal_bytes_with_secrets;
use dioxus::logger::tracing::error;
use dioxus::prelude::*;
use dioxus_free_icons::{
    icons::fa_solid_icons::{FaChevronDown, FaComments, FaPlus},
    Icon,
};
use web_sys::window;

// Access the build timestamp (ISO 8601 format) environment variable set by build.rs
const BUILD_TIMESTAMP_ISO: &str = env!("BUILD_TIMESTAMP_ISO", "Build timestamp not set");

/// Convert UTC ISO timestamp to local time string
fn format_build_time_local() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        use js_sys::Date;
        let date = Date::new(&wasm_bindgen::JsValue::from_str(BUILD_TIMESTAMP_ISO));
        if date.to_string().as_string().is_some() {
            // Format as "YYYY-MM-DD HH:MM" in local time
            let year = date.get_full_year();
            let month = date.get_month() + 1; // JS months are 0-indexed
            let day = date.get_date();
            let hours = date.get_hours();
            let minutes = date.get_minutes();
            let offset_min = date.get_timezone_offset() as i32;
            let tz_str = if offset_min == 0 {
                "UTC".to_string()
            } else {
                // getTimezoneOffset returns minutes west of UTC, so negate
                let sign = if offset_min <= 0 { '+' } else { '-' };
                let abs = offset_min.unsigned_abs();
                format!("UTC{}{}", sign, abs / 60)
            };
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02} {}",
                year, month, day, hours, minutes, tz_str
            )
        } else {
            BUILD_TIMESTAMP_ISO.to_string()
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        BUILD_TIMESTAMP_ISO.to_string()
    }
}

#[component]
pub fn RoomList() -> Element {
    let mut is_open = use_signal(|| false);

    // Memoize the room list to avoid reading signals during render
    let room_items = use_memo(move || {
        let rooms = ROOMS.read();
        let current_room_key = CURRENT_ROOM.read().owner_key;

        rooms
            .map
            .iter()
            .map(|(room_key, room_data)| {
                let room_key = *room_key;
                // Decrypt room name if room is private and we have the secret
                let sealed_name = &room_data
                    .room_state
                    .configuration
                    .configuration
                    .display
                    .name;
                let room_name = match unseal_bytes_with_secrets(sealed_name, &room_data.secrets) {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                    Err(_) => sealed_name.to_string_lossy(),
                };
                let is_current = current_room_key == Some(room_key);
                (room_key, room_name, is_current)
            })
            .collect::<Vec<_>>()
    });

    // Get current room name for the dropdown button
    let current_room_name = use_memo(move || {
        room_items
            .read()
            .iter()
            .find(|(_, _, is_current)| *is_current)
            .map(|(_, name, _)| name.clone())
            .unwrap_or_else(|| "Select Board".to_string())
    });

    rsx! {
        div { class: "relative inline-block items-center",
            // Dropdown trigger button
            div { class: "flex justify-center", style: "width: 100vw;",
                button {
                    class: "flex items-center gap-6 px-4 py-2 bg-panel text-sm text-text hover:bg-surface transition-colors min-w-48 min-h-24", style: "min-width: 60vw;",
                    onclick: move |_| {
                        is_open.set(!is_open());
                    },
                    Icon { width: 48, height: 48, icon: FaComments, class: "text-text-muted" }
                    span { class: "flex-1 text-left text-4xl truncate", "{current_room_name}" }
                    Icon {
                        width: 12,
                        height: 12,
                        icon: FaChevronDown,
                        class: format!("text-text-muted transition-transform {}", if is_open() { "rotate-180" } else { "" })
                    }
                }

                // Dropdown panel
                if is_open() {
                    // Backdrop to close dropdown when clicking outside
                    div {
                        class: "fixed inset-0 z-10",
                        onclick: move |_| {
                            is_open.set(false);
                        }
                    }

                    // Dropdown menu
                    div { class: "flex justify-center absolute left-0 top-full overflow-hidden z-20", style: "width: 100vw",
                        div { class: "bg-panel", style: "min-width: 60vw",
                            // Board list
                            ul { class: "max-h-64 overflow-y-auto py-1",
                                {room_items.read().iter().map(|(room_key, room_name, is_current)| {
                                    let room_key = *room_key;
                                    let room_name = room_name.clone();
                                    let is_current = *is_current;
                                    rsx! {
                                        li { key: "{room_key:?}",
                                            button {
                                                class: format!(
                                                    "w-full text-left px-3 py-2 text-sm transition-colors {}",
                                                    if is_current {
                                                        "bg-accent/10 text-accent font-medium"
                                                    } else {
                                                        "text-text hover:bg-surface"
                                                    }
                                                ),
                                                onclick: move |_| {
                                                    // Navigate to room URL using hash (component is outside Router context)
                                                    let room_id = bs58::encode(room_key.as_bytes()).into_string();
                                                    if let Some(win) = window() {
                                                        let _ = win.location().set_hash(&format!("/room/{}", room_id));
                                                    }
                                                    mark_current_room_as_read();
                                                    is_open.set(false);
                                                    spawn(async move {
                                                        if let Err(e) = save_rooms_to_delegate().await {
                                                            error!("Failed to save current room selection: {}", e);
                                                        }
                                                    });
                                                },
                                                span { class: "block truncate", "{room_name}" }
                                            }
                                        }
                                    }
                                }).collect::<Vec<_>>().into_iter()}

                                // Empty state
                                if room_items.read().is_empty() {
                                    li { class: "px-3 py-4 text-sm text-text-muted text-center",
                                        "No boards yet"
                                    }
                                }
                            }

                            // Header with create room button
                            div { class: "px-3 py-2 border-t border-border flex items-center justify-center",
                                span { class: "text-xs font-semibold text-text-muted uppercase tracking-wide",
                                    "Boards"
                                }
                                button {
                                    class: "p-1 rounded text-text-muted hover:text-accent hover:bg-surface transition-colors",
                                    title: "Create Board",
                                    onclick: move |_| {
                                        CREATE_ROOM_MODAL.write().show = true;
                                        is_open.set(false);
                                    },
                                    Icon { width: 12, height: 12, icon: FaPlus }
                                }
                            }

                            // Build info footer
                            div { class: "px-3 py-2 border-t border-border text-xs text-text-muted text-center",
                                {"Built: "} {format_build_time_local()}
                            }
                        }
                    }
                }
            }
        }
    }
}
