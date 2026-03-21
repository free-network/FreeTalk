use super::board_name_field::BoardNameField;
use crate::components::app::chat_delegate::save_boards_to_delegate;
use crate::components::app::{mark_needs_sync, BOARDS, CURRENT_BOARD, EDIT_BOARD_MODAL};
use dioxus::logger::tracing::{error, info};
use dioxus::prelude::*;
use dioxus_free_icons::icons::fa_solid_icons::FaRotate;
use dioxus_free_icons::Icon;
use freenet_scaffold::ComposableState;
use river_core::board_state::privacy::PrivacyMode;
use river_core::board_state::{ChatBoardParametersV1, ChatBoardStateV1Delta};
use std::ops::Deref;

#[component]
pub fn EditBoardModal() -> Element {
    // State for leave confirmation
    let mut show_leave_confirmation = use_signal(|| false);

    // Memoize the board being edited
    let editing_board = use_memo(move || {
        EDIT_BOARD_MODAL.read().board.and_then(|editing_board_vk| {
            BOARDS.read().map.iter().find_map(|(board_vk, board_data)| {
                if &editing_board_vk == board_vk {
                    Some(board_data.clone())
                } else {
                    None
                }
            })
        })
    });

    // Memoize the board configuration
    let board_config = use_memo(move || {
        editing_board
            .read()
            .as_ref()
            .map(|board_data| board_data.board_state.configuration.configuration.clone())
    });

    // Memoize if the current user is the owner of the board being edited
    let user_is_owner = use_memo(move || {
        editing_board.read().as_ref().is_some_and(|board_data| {
            let user_vk = board_data.self_sk.verifying_key();
            let board_vk = EDIT_BOARD_MODAL.read().board.unwrap();
            user_vk == board_vk
        })
    });

    // Render the modal if board configuration is available
    if let Some(config) = board_config.clone().read().deref() {
        rsx! {
            // Modal backdrop
            div {
                class: "fixed inset-0 z-50 flex items-center justify-center",
                // Overlay
                div {
                    class: "absolute inset-0 bg-black/50",
                    onclick: move |_| {
                        EDIT_BOARD_MODAL.write().board = None;
                    }
                }
                // Modal content
                div {
                    class: "relative z-10 w-full max-w-md mx-4 bg-panel rounded-xl shadow-xl border border-border max-h-[90vh] overflow-y-auto",
                    div {
                        class: "p-6",
                        h1 { class: "text-xl font-semibold text-text mb-4", "Board Details" }

                        BoardNameField {
                            config: config.clone(),
                            is_owner: *user_is_owner.read()
                        }

                        // Read-only board info
                        if let Some(board_data) = editing_board.read().as_ref() {
                            // Board Public Key
                            div {
                                class: "mt-4",
                                label {
                                    class: "block text-sm font-medium text-text-muted mb-1",
                                    title: "Ed25519 public key (Curve25519 elliptic curve)",
                                    "Board Public Key"
                                }
                                input {
                                    r#type: "text",
                                    readonly: true,
                                    title: "Ed25519 public key (Curve25519 elliptic curve)",
                                    class: "w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-muted text-sm font-mono cursor-text select-all",
                                    value: "{bs58::encode(board_data.owner_vk.as_bytes()).into_string()}"
                                }
                            }
                            // Contract ID
                            div {
                                class: "mt-4",
                                label {
                                    class: "block text-sm font-medium text-text-muted mb-1",
                                    "Contract ID"
                                }
                                input {
                                    r#type: "text",
                                    readonly: true,
                                    class: "w-full px-3 py-2 bg-surface border border-border rounded-lg text-text-muted text-sm font-mono cursor-text select-all",
                                    value: "{board_data.contract_key.id()}"
                                }
                            }

                            // Secret Version (only for private boards)
                            {
                                let is_private = board_data.board_state.configuration.configuration.privacy_mode == PrivacyMode::Private;
                                let is_owner = board_data.owner_vk == board_data.self_sk.verifying_key();
                                let secret_version = board_data.board_state.secrets.current_version;

                                if is_private {
                                    Some(rsx! {
                                        div {
                                            class: "mt-4",
                                            label {
                                                class: "block text-sm font-medium text-text-muted mb-1",
                                                "Secret Version"
                                            }
                                            div {
                                                class: "flex items-center gap-2",
                                                input {
                                                    r#type: "text",
                                                    readonly: true,
                                                    class: "flex-1 px-3 py-2 bg-surface border border-border rounded-lg text-text-muted text-sm font-mono cursor-text select-all",
                                                    value: "{secret_version}"
                                                }
                                                {
                                                    if is_owner {
                                                        Some(rsx! {
                                                            button {
                                                                class: "px-3 py-2 bg-surface hover:bg-surface-hover border border-border rounded-lg text-text-muted hover:text-text transition-colors flex items-center gap-2",
                                                                title: "Rotate board secret - generates a new encryption key for future messages",
                                                                onclick: move |_| {
                                                                    if let Some(current_board) = EDIT_BOARD_MODAL.read().board {
                                                                        info!("Rotating secret for board");
                                                                        BOARDS.with_mut(|boards| {
                                                                            if let Some(board_data) = boards.map.get_mut(&current_board) {
                                                                                match board_data.rotate_secret() {
                                                                                    Ok(secrets_delta) => {
                                                                                        info!("Secret rotated successfully");
                                                                                        let current_state = board_data.board_state.clone();
                                                                                        let delta = ChatBoardStateV1Delta {
                                                                                            secrets: Some(secrets_delta),
                                                                                            ..Default::default()
                                                                                        };
                                                                                        if let Err(e) = board_data.board_state.apply_delta(
                                                                                            &current_state,
                                                                                            &ChatBoardParametersV1 { owner: current_board },
                                                                                            &Some(delta),
                                                                                        ) {
                                                                                            error!("Failed to apply rotation delta: {}", e);
                                                                                        } else {
                                                                                            mark_needs_sync(current_board);
                                                                                        }
                                                                                    }
                                                                                    Err(e) => error!("Failed to rotate secret: {}", e),
                                                                                }
                                                                            }
                                                                        });
                                                                    }
                                                                },
                                                                Icon { icon: FaRotate, width: 14, height: 14 }
                                                                span { "Rotate" }
                                                            }
                                                        })
                                                    } else {
                                                        None
                                                    }
                                                }
                                            }
                                        }
                                    })
                                } else {
                                    None
                                }
                            }
                        }

                        // Leave Board Section
                        if *show_leave_confirmation.read() {
                            div {
                                class: "bg-yellow-500/10 border border-yellow-500/20 rounded-lg p-4 mt-4",
                                p {
                                    class: "text-yellow-400 mb-3",
                                    if *user_is_owner.read() {
                                        "Warning: You are the owner of this board. Leaving will permanently delete it for you. Other members might retain access if they have the contract key, but coordination will be lost."
                                    } else {
                                        "Are you sure you want to leave this board? This action cannot be undone."
                                    }
                                }
                                div {
                                    class: "flex gap-3",
                                    button {
                                        class: "px-4 py-2 bg-red-500 hover:bg-red-600 text-white font-medium rounded-lg transition-colors",
                                        onclick: move |_| {
                                            // Read the board_vk first and drop the read borrow
                                            let board_vk_to_remove = EDIT_BOARD_MODAL.read().board;

                                            if let Some(board_vk) = board_vk_to_remove {
                                                // Perform writes *after* the read borrow is dropped
                                                BOARDS.write().map.remove(&board_vk);

                                                // Check and potentially clear CURRENT_BOARD
                                                if CURRENT_BOARD.read().owner_key == Some(board_vk) {
                                                    CURRENT_BOARD.write().owner_key = None;
                                                }

                                                // Close the modal *last*
                                                EDIT_BOARD_MODAL.write().board = None;

                                                // Save updated boards to delegate storage
                                                info!("Board removed, saving to delegate");
                                                spawn(async move {
                                                    if let Err(e) = save_boards_to_delegate().await {
                                                        error!("Failed to save boards after removal: {}", e);
                                                    }
                                                });
                                            }
                                            // Reset confirmation state regardless
                                            show_leave_confirmation.set(false);
                                        },
                                        "Confirm Leave"
                                    }
                                    button {
                                        class: "px-4 py-2 bg-surface hover:bg-surface-hover text-text rounded-lg transition-colors",
                                        onclick: move |_| show_leave_confirmation.set(false),
                                        "Cancel"
                                    }
                                }
                            }
                        } else {
                             // Only show Leave button if not confirming
                            div {
                                class: "mt-4",
                                button {
                                    class: "px-4 py-2 border border-red-500 text-red-500 hover:bg-red-500/10 rounded-lg transition-colors",
                                    onclick: move |_| show_leave_confirmation.set(true),
                                    "Leave Board"
                                }
                            }
                        }
                    }
                    // Close button
                    button {
                        class: "absolute top-3 right-3 p-1 text-text-muted hover:text-text transition-colors",
                        onclick: move |_| {
                            EDIT_BOARD_MODAL.write().board = None;
                        },
                        "✕"
                    }
                }
            }
        }
    } else {
        rsx! {}
    }
}
