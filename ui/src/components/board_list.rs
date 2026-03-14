pub(crate) mod board_name_field;
pub(crate) mod create_board_modal;
pub(crate) mod edit_board_modal;
pub(crate) mod receive_invitation_modal;

use crate::components::app::chat_delegate::save_boards_to_delegate;
use crate::components::app::document_title::mark_current_board_as_read;
use crate::components::app::{BOARDS, CREATE_BOARD_MODAL, CURRENT_BOARD};
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
pub fn BoardList() -> Element {
    let mut is_open = use_signal(|| false);

    // Memoize the board list to avoid reading signals during render
    let board_items = use_memo(move || {
        let boards = BOARDS.read();
        let current_board_key = CURRENT_BOARD.read().owner_key;

        boards
            .map
            .iter()
            .map(|(board_key, board_data)| {
                let board_key = *board_key;
                // Decrypt board name if board is private and we have the secret
                let sealed_name = &board_data
                    .board_state
                    .configuration
                    .configuration
                    .display
                    .name;
                let board_name = match unseal_bytes_with_secrets(sealed_name, &board_data.secrets) {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                    Err(_) => sealed_name.to_string_lossy(),
                };
                let is_current = current_board_key == Some(board_key);
                (board_key, board_name, is_current)
            })
            .collect::<Vec<_>>()
    });

    // Get current board name for the dropdown button
    let current_board_name = use_memo(move || {
        board_items
            .read()
            .iter()
            .find(|(_, _, is_current)| *is_current)
            .map(|(_, name, _)| name.clone())
            .unwrap_or_else(|| "Select Board".to_string())
    });

    rsx! {
        div { class: "relative w-full",
            // Dropdown trigger button - current board display
            div { class: "flex justify-center",
                button {
                    class: "flex items-center gap-6 px-6 py-4 bg-panel text-text hover:bg-surface transition-colors",
                    style: "min-width: 60vw;",
                    onclick: move |_| {
                        is_open.set(!is_open());
                    },
                    Icon { width: 48, height: 48, icon: FaComments, class: "text-text-muted" }
                    span { class: "flex-1 text-left text-4xl font-medium truncate", "{current_board_name}" }
                    Icon {
                        width: 16,
                        height: 16,
                        icon: FaChevronDown,
                        class: format!("text-text-muted transition-transform {}", if is_open() { "rotate-180" } else { "" })
                    }
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

                // Dropdown menu - centered like the button
                div { class: "absolute left-0 right-0 flex justify-center z-20",
                    div { class: "bg-panel shadow-lg rounded-b-xl overflow-hidden",
                        style: "min-width: 60vw;",

                        // Board list
                        div { class: "max-h-96 overflow-y-auto",
                            {board_items.read().iter().map(|(board_key, board_name, is_current)| {
                                let board_key = *board_key;
                                let board_name = board_name.clone();
                                let is_current = *is_current;
                                rsx! {
                                    button {
                                        key: "{board_key:?}",
                                        class: format!(
                                            "w-full flex items-center gap-4 px-6 py-4 transition-colors {}",
                                            if is_current {
                                                "bg-accent/10 text-accent"
                                            } else {
                                                "text-text hover:bg-surface"
                                            }
                                        ),
                                        onclick: move |_| {
                                            // Navigate to board URL using hash (component is outside Router context)
                                            let board_id = bs58::encode(board_key.as_bytes()).into_string();
                                            if let Some(win) = window() {
                                                let _ = win.location().set_hash(&format!("/board/{}", board_id));
                                            }
                                            mark_current_board_as_read();
                                            is_open.set(false);
                                            spawn(async move {
                                                if let Err(e) = save_boards_to_delegate().await {
                                                    error!("Failed to save current board selection: {}", e);
                                                }
                                            });
                                        },
                                        Icon {
                                            width: 32,
                                            height: 32,
                                            icon: FaComments,
                                            class: if is_current { "text-accent" } else { "text-text-muted" }
                                        }
                                        span { class: "flex-1 text-left text-2xl truncate", "{board_name}" }
                                    }
                                }
                            }).collect::<Vec<_>>().into_iter()}

                            // Empty state
                            if board_items.read().is_empty() {
                                div { class: "px-6 py-8 text-xl text-text-muted text-center",
                                    "No boards yet"
                                }
                            }
                        }

                        // Create board button
                        button {
                            class: "w-full flex items-center gap-4 px-6 py-4 border-t border-border text-text-muted hover:text-accent hover:bg-surface transition-colors",
                            onclick: move |_| {
                                CREATE_BOARD_MODAL.write().show = true;
                                is_open.set(false);
                            },
                            Icon { width: 32, height: 32, icon: FaPlus }
                            span { class: "text-2xl", "Create Board" }
                        }

                        // Build info footer
                        div { class: "px-6 py-2 border-t border-border text-sm text-text-muted text-center",
                            {"Built: "} {format_build_time_local()}
                        }
                    }
                }
            }
        }
    }
}
