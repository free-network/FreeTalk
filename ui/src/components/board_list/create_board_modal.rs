use crate::components::app::{BOARDS, CREATE_BOARD_MODAL, NEEDS_SYNC};
use dioxus::prelude::*;
use ed25519_dalek::SigningKey;
use web_sys::window;

#[component]
pub fn CreateBoardModal() -> Element {
    let mut board_name = use_signal(String::new);
    let mut nickname = use_signal(String::new);
    let mut error_msg = use_signal(|| None::<String>);

    let create_board = move |_| {
        use dioxus::logger::tracing::info;
        info!("🔵 Create board button clicked");

        let name = board_name.read().clone();
        if name.trim().is_empty() {
            error_msg.set(Some("Board name cannot be empty".to_string()));
            return;
        }

        let nick = nickname.read().clone();
        if nick.trim().is_empty() {
            error_msg.set(Some("Nickname cannot be empty".to_string()));
            return;
        }

        error_msg.set(None);
        info!("🔵 Board name: {}", name);

        // Generate key outside the borrow
        info!("🔵 Generating signing key...");
        let self_sk = SigningKey::generate(&mut rand::thread_rng());
        let private = false; // Private boards temporarily disabled
        info!(
            "🔵 Creating {} board with nickname: {}",
            if private { "private" } else { "public" },
            nick
        );

        // Create board and get the key
        info!("🔵 About to call create_new_board_with_name...");
        let new_board_key = BOARDS
            .with_mut(|boards| boards.create_new_board_with_name(self_sk, name, nick, private));
        info!("🔵 Board created with key: {:?}", new_board_key);

        // Navigate to board URL using hash (modal is outside Router context)
        info!("🔵 Navigating to new board...");
        let board_id = bs58::encode(new_board_key.as_bytes()).into_string();
        if let Some(win) = window() {
            let _ = win.location().set_hash(&format!("/board/{}", board_id));
        }
        info!("🔵 Navigation triggered");

        // Mark board as needing sync (this will trigger use_effect in app.rs)
        info!("🔵 Marking board for synchronization...");
        NEEDS_SYNC.write().insert(new_board_key);
        info!("🔵 Board marked for sync");

        // Reset and close modal
        info!("🔵 Resetting form fields...");
        board_name.set(String::new());
        nickname.set(String::new());
        info!("🔵 Closing modal...");
        CREATE_BOARD_MODAL.with_mut(|modal| {
            modal.show = false;
        });
        info!("🔵 Modal closed");
        info!("🔵 Create board handler completed successfully");
    };

    let is_open = CREATE_BOARD_MODAL.read().show;

    if !is_open {
        return rsx! {};
    }

    rsx! {
        // Backdrop
        div {
            class: "fixed inset-0 bg-black/50 z-40",
            onclick: move |_| {
                CREATE_BOARD_MODAL.with_mut(|modal| {
                    modal.show = false;
                });
            }
        }

        // Modal
        div { class: "fixed inset-0 z-50 flex items-center justify-center p-4",
            div {
                class: "bg-panel rounded-xl shadow-xl max-w-md w-full",
                onclick: move |e| e.stop_propagation(),

                // Header
                div { class: "px-6 py-4 border-b border-border",
                    h2 { class: "text-lg font-semibold text-text", "Create New Board" }
                }

                // Body
                div { class: "px-6 py-4 space-y-4",
                    // Error message
                    if let Some(err) = error_msg.read().as_ref() {
                        div { class: "p-3 bg-error-bg border border-red-200 dark:border-red-800 rounded-lg text-sm text-red-700 dark:text-red-400",
                            "{err}"
                        }
                    }

                    div {
                        label { class: "block text-sm font-medium text-text mb-1", "Board Name" }
                        input {
                            class: "w-full px-3 py-2 bg-surface border border-border rounded-lg text-text placeholder-text-muted focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent",
                            value: "{board_name}",
                            placeholder: "Enter board name",
                            onchange: move |evt| board_name.set(evt.value().to_string())
                        }
                    }

                    div {
                        label { class: "block text-sm font-medium text-text mb-1", "Your Nickname" }
                        input {
                            class: "w-full px-3 py-2 bg-surface border border-border rounded-lg text-text placeholder-text-muted focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent",
                            value: "{nickname}",
                            placeholder: "Enter your nickname",
                            onchange: move |evt| nickname.set(evt.value().to_string())
                        }
                    }

                    label { class: "flex items-center gap-3 cursor-not-allowed opacity-50",
                        input {
                            r#type: "checkbox",
                            class: "w-4 h-4 rounded border-border text-accent focus:ring-accent/50",
                            checked: false,
                            disabled: true,
                        }
                        span { class: "text-sm text-text-muted",
                            "Private boards temporarily disabled"
                        }
                    }
                }

                // Footer
                div { class: "px-6 py-4 border-t border-border flex justify-end gap-3",
                    button {
                        class: "px-4 py-2 text-sm text-text-muted hover:text-text hover:bg-surface rounded-lg transition-colors",
                        onclick: move |_| {
                            CREATE_BOARD_MODAL.with_mut(|modal| {
                                modal.show = false;
                            });
                        },
                        "Cancel"
                    }
                    button {
                        class: "px-4 py-2 bg-accent hover:bg-accent-hover text-white text-sm font-medium rounded-lg transition-colors",
                        onclick: create_board,
                        "Create Board"
                    }
                }
            }
        }
    }
}
