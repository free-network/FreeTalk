use crate::board_data::BoardData;
use crate::components::app::{BOARDS, CURRENT_BOARD, MEMBER_INFO_MODAL, NEEDS_SYNC};
use crate::util::get_current_system_time;
use dioxus::logger::tracing::{error, info};
use dioxus::prelude::*;
use freenet_scaffold::ComposableState;
use river_core::board_state::ban::{AuthorizedUserBan, UserBan};
use river_core::board_state::member::MemberId;
use river_core::board_state::{ChatBoardParametersV1, ChatBoardStateV1Delta};
use wasm_bindgen_futures::spawn_local;

#[component]
pub fn BanButton(member_to_ban: MemberId, is_downstream: bool, nickname: String) -> Element {
    // Memos
    let current_board_data_signal: Memo<Option<BoardData>> = use_memo(move || {
        CURRENT_BOARD
            .read()
            .owner_key
            .as_ref()
            .and_then(|key| BOARDS.read().map.get(key).cloned())
    });

    let mut show_confirmation = use_signal(|| false);

    let execute_ban = move |_| {
        if let (Some(current_board), Some(board_data)) = (
            CURRENT_BOARD.read().owner_key,
            current_board_data_signal.read().as_ref(),
        ) {
            let board_key = board_data.board_key();
            let self_sk = board_data.self_sk.clone();
            let board_state_clone = board_data.board_state.clone();
            let banned_by = MemberId::from(&self_sk.verifying_key());

            let ban = UserBan {
                owner_member_id: MemberId::from(&current_board),
                banned_at: get_current_system_time(),
                banned_user: member_to_ban,
            };

            // Close modal immediately for better UX
            MEMBER_INFO_MODAL.with_mut(|modal| {
                modal.member = None;
            });

            spawn_local(async move {
                // Serialize ban to CBOR for signing
                let mut ban_bytes = Vec::new();
                if let Err(e) = ciborium::ser::into_writer(&ban, &mut ban_bytes) {
                    error!("Failed to serialize ban for signing: {:?}", e);
                    return;
                }

                // Sign using delegate with fallback to local signing
                let signature =
                    crate::signing::sign_ban_with_fallback(board_key, ban_bytes, &self_sk).await;

                let authorized_ban = AuthorizedUserBan::with_signature(ban, banned_by, signature);

                let delta = ChatBoardStateV1Delta {
                    bans: Some(vec![authorized_ban]),
                    ..Default::default()
                };

                BOARDS.with_mut(|boards| {
                    if let Some(board_data_mut) = boards.map.get_mut(&current_board) {
                        if let Err(e) = board_data_mut.board_state.apply_delta(
                            &board_state_clone,
                            &ChatBoardParametersV1 {
                                owner: current_board,
                            },
                            &Some(delta),
                        ) {
                            error!("Failed to apply ban delta: {:?}", e);
                        } else {
                            info!("Successfully applied ban delta for member {:?}", member_to_ban);

                            // If this is a private board and we're the owner, rotate the secret
                            // This ensures the banned member cannot decrypt future messages
                            if board_data_mut.is_private() && board_data_mut.owner_vk == board_data_mut.self_sk.verifying_key() {
                                info!("Private board - rotating secret after ban to ensure forward secrecy");

                                match board_data_mut.rotate_secret() {
                                    Ok(secrets_delta) => {
                                        info!("Secret rotated successfully after ban, applying delta");

                                        // Apply the secrets delta
                                        let current_state = board_data_mut.board_state.clone();
                                        let rotation_delta = ChatBoardStateV1Delta {
                                            secrets: Some(secrets_delta),
                                            ..Default::default()
                                        };

                                        if let Err(e) = board_data_mut.board_state.apply_delta(
                                            &current_state,
                                            &ChatBoardParametersV1 { owner: current_board },
                                            &Some(rotation_delta),
                                        ) {
                                            error!("Failed to apply rotation delta after ban: {}", e);
                                        } else {
                                            info!("Secret rotation applied after ban");
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to rotate secret after ban: {}", e);
                                    }
                                }
                            }
                        }
                    }
                });

                // Mark board as needing sync to propagate ban and rotation
                NEEDS_SYNC.write().insert(current_board);
                info!("Marked board for synchronization after ban");
            });
        }
    };

    if is_downstream {
        rsx! {
            div { class: "mt-4",
                button {
                    class: "px-4 py-2 bg-red-500 hover:bg-red-600 text-white font-medium rounded-lg transition-colors",
                    onclick: move |_| show_confirmation.set(true),
                    "Ban User"
                }

                if *show_confirmation.read() {
                    // Confirmation modal
                    div {
                        class: "fixed inset-0 z-50 flex items-center justify-center",
                        // Overlay
                        div {
                            class: "absolute inset-0 bg-black/50",
                            onclick: move |_| show_confirmation.set(false)
                        }
                        // Modal content
                        div {
                            class: "relative z-10 w-full max-w-md mx-4 bg-panel rounded-xl shadow-xl border border-border",
                            // Header
                            div {
                                class: "px-6 py-4 border-b border-border flex items-center justify-between",
                                h2 { class: "text-lg font-semibold text-text", "Confirm Ban" }
                                button {
                                    class: "p-1 text-text-muted hover:text-text transition-colors",
                                    onclick: move |_| show_confirmation.set(false),
                                    "✕"
                                }
                            }

                            // Body
                            div {
                                class: "px-6 py-4",
                                p { class: "text-text",
                                    "Are you sure you want to ban "
                                    span { class: "font-semibold", "{nickname}" }
                                    " (ID: "
                                    code { class: "text-sm bg-surface px-1 rounded", "{member_to_ban}" }
                                    ")? This action cannot be undone."
                                }
                            }

                            // Footer
                            div {
                                class: "px-6 py-4 border-t border-border flex justify-end gap-3",
                                button {
                                    class: "px-4 py-2 bg-surface hover:bg-surface-hover text-text rounded-lg transition-colors",
                                    onclick: move |_| show_confirmation.set(false),
                                    "Cancel"
                                }
                                button {
                                    class: "px-4 py-2 bg-red-500 hover:bg-red-600 text-white font-medium rounded-lg transition-colors",
                                    onclick: execute_ban,
                                    "Yes, Ban User"
                                }
                            }
                        }
                    }
                }
            }
        }
    } else {
        rsx! { "" }
    }
}
