//! Admin management view for viewing and adding admins.

use crate::board_data::BoardData;
use crate::components::app::{BOARDS, CURRENT_BOARD, NEEDS_SYNC};
use dioxus::logger::tracing::{error, info};
use dioxus::prelude::*;
use freenet_scaffold::util::FastHash;
use freenet_scaffold::ComposableState;
use river_core::board_state::admin::{Admin, AdminsDelta, AuthorizedAdmin};
use river_core::board_state::member::MemberId;
use river_core::board_state::{ChatBoardParametersV1, ChatBoardStateV1Delta};
use wasm_bindgen_futures::spawn_local;

#[component]
pub fn AdminView() -> Element {
    let current_board_data: Memo<Option<BoardData>> = use_memo(move || {
        CURRENT_BOARD
            .read()
            .owner_key
            .as_ref()
            .and_then(|key| BOARDS.read().map.get(key).cloned())
    });

    let mut selected_member = use_signal(|| None::<MemberId>);
    let mut show_add_form = use_signal(|| false);

    // Check if current user is the owner
    let is_owner = use_memo(move || {
        current_board_data
            .read()
            .as_ref()
            .map_or(false, |board_data| {
                board_data.owner_vk == board_data.self_sk.verifying_key()
            })
    });

    // Get list of current admins
    let admins = use_memo(move || {
        current_board_data
            .read()
            .as_ref()
            .map(|board_data| board_data.board_state.admin.admins.clone())
            .unwrap_or_default()
    });

    // Get list of banned users
    let banned_users = use_memo(move || {
        current_board_data
            .read()
            .as_ref()
            .map(|board_data| board_data.board_state.bans.0.clone())
            .unwrap_or_default()
    });

    // Get list of members who can be made admin (not already admin, not owner)
    let available_members = use_memo(move || {
        current_board_data
            .read()
            .as_ref()
            .map_or(vec![], |board_data| {
                let owner_id = MemberId::from(&board_data.owner_vk);
                let admin_ids: std::collections::HashSet<_> = board_data
                    .board_state
                    .admin
                    .admins
                    .iter()
                    .map(|a| a.admin.member_id)
                    .collect();

                board_data
                    .board_state
                    .members
                    .members
                    .iter()
                    .filter(|m| {
                        let member_id = m.member.id();
                        member_id != owner_id && !admin_ids.contains(&member_id)
                    })
                    .map(|m| m.member.id())
                    .collect::<Vec<_>>()
            })
    });

    // Helper to get nickname for a member
    let get_nickname = move |member_id: MemberId| -> String {
        current_board_data
            .read()
            .as_ref()
            .and_then(|board_data| {
                board_data
                    .board_state
                    .member_info
                    .member_info
                    .iter()
                    .find(|info| info.member_info.member_id == member_id)
                    .map(|info| info.member_info.preferred_nickname.to_string_lossy())
            })
            .unwrap_or_else(|| format!("{}", member_id))
    };

    let add_admin = move |_| {
        let Some(member_id) = *selected_member.read() else {
            return;
        };

        let Some(board_data) = current_board_data.read().as_ref().cloned() else {
            return;
        };

        let current_board = board_data.owner_vk;
        let board_key = board_data.board_key();
        let self_sk = board_data.self_sk.clone();
        let board_state_clone = board_data.board_state.clone();

        let admin = Admin { member_id };

        // Close form and reset selection
        show_add_form.set(false);
        selected_member.set(None);

        spawn_local(async move {
            // Serialize admin to CBOR for signing
            let mut admin_bytes = Vec::new();
            if let Err(e) = ciborium::ser::into_writer(&admin, &mut admin_bytes) {
                error!("Failed to serialize admin for signing: {:?}", e);
                return;
            }

            // Sign using delegate with fallback to local signing
            let signature =
                crate::signing::sign_admin_with_fallback(board_key, admin_bytes, &self_sk).await;

            let authorized_admin = AuthorizedAdmin::with_signature(admin, signature);

            let delta = ChatBoardStateV1Delta {
                admin: Some(AdminsDelta::new(vec![authorized_admin])),
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
                        error!("Failed to apply admin delta: {:?}", e);
                    } else {
                        info!("Successfully added admin {:?}", member_id);
                    }
                }
            });

            NEEDS_SYNC.write().insert(current_board);
            info!("Marked board for synchronization after adding admin");
        });
    };

    // If no board selected, show message
    if current_board_data.read().is_none() {
        return rsx! {
            div { class: "flex-1 flex items-center justify-center p-4",
                p { class: "text-text-muted", "Select a board to manage admins" }
            }
        };
    }

    rsx! {
        div { class: "flex-1 flex flex-col p-6 overflow-y-auto",
            // Header
            h1 { class: "text-2xl font-bold text-text mb-6", "Admin Management" }

            // Warning banner
            div { class: "bg-amber-500/20 border border-amber-500/50 rounded-lg p-4 mb-6",
                div { class: "flex items-start gap-3",
                    span { class: "text-amber-500 text-xl", "⚠" }
                    div {
                        p { class: "text-amber-200 font-semibold", "Important: Admins cannot be removed" }
                        p { class: "text-amber-200/80 text-sm mt-1",
                            "Once a member is made an admin, this action cannot be undone. "
                            "Please carefully consider before adding someone as an admin."
                        }
                    }
                }
            }

            // Current admins section
            div { class: "mb-6",
                h2 { class: "text-lg font-semibold text-text mb-3", "Current Admins" }

                if admins.read().is_empty() {
                    div { class: "bg-surface rounded-lg p-4 text-text-muted",
                        "No admins have been added yet. The board owner has full admin privileges by default."
                    }
                } else {
                    div { class: "space-y-2",
                        for admin in admins.read().iter() {
                            div { class: "bg-surface rounded-lg p-3 flex items-center gap-3",
                                div { class: "w-8 h-8 bg-accent/20 rounded-full flex items-center justify-center text-accent text-sm",
                                    {get_nickname(admin.admin.member_id).chars().next().unwrap_or('?').to_string()}
                                }
                                div {
                                    p { class: "text-text font-medium", "{get_nickname(admin.admin.member_id)}" }
                                    p { class: "text-text-muted text-xs", "{admin.admin.member_id}" }
                                }
                            }
                        }
                    }
                }
            }

            // Banned users section
            div { class: "mb-6",
                h2 { class: "text-lg font-semibold text-text mb-3", "Banned Users" }

                if banned_users.read().is_empty() {
                    div { class: "bg-surface rounded-lg p-4 text-text-muted",
                        "No users have been banned."
                    }
                } else {
                    div { class: "space-y-2",
                        for ban in banned_users.read().iter() {
                            div { class: "bg-surface rounded-lg p-3 flex items-center gap-3",
                                div { class: "w-8 h-8 bg-red-500/20 rounded-full flex items-center justify-center text-red-400 text-sm",
                                    {get_nickname(ban.ban.banned_user).chars().next().unwrap_or('?').to_string()}
                                }
                                div { class: "flex-1",
                                    p { class: "text-text font-medium", "{get_nickname(ban.ban.banned_user)}" }
                                    p { class: "text-text-muted text-xs", "{ban.ban.banned_user}" }
                                }
                                div { class: "text-right text-xs text-text-muted",
                                    p { "Banned by: {get_nickname(ban.banned_by)}" }
                                }
                            }
                        }
                    }
                }
            }

            // Add admin section (owner only)
            if *is_owner.read() {
                div { class: "border-t border-border pt-6",
                    h2 { class: "text-lg font-semibold text-text mb-3", "Add Admin" }

                    if !*show_add_form.read() {
                        button {
                            class: "px-4 py-2 bg-accent hover:bg-accent-hover text-white rounded-lg transition-colors",
                            onclick: move |_| show_add_form.set(true),
                            "+ Add Admin"
                        }
                    } else {
                        div { class: "bg-surface rounded-lg p-4 space-y-4",
                            if available_members.read().is_empty() {
                                p { class: "text-text-muted",
                                    "No members available to add as admin. All members are either already admins or the owner."
                                }
                            } else {
                                // Member selection dropdown
                                div {
                                    label { class: "block text-sm text-text-muted mb-2", "Select Member" }
                                    select {
                                        class: "w-full bg-panel border border-border rounded-lg px-3 py-2 text-text focus:outline-none focus:ring-2 focus:ring-accent",
                                        onchange: move |evt| {
                                            if let Ok(hash_value) = evt.value().parse::<i64>() {
                                                selected_member.set(Some(MemberId(FastHash(hash_value))));
                                            }
                                        },
                                        option { value: "", "-- Select a member --" }
                                        for member_id in available_members.read().iter() {
                                            option {
                                                value: "{member_id.0.0}",
                                                "{get_nickname(*member_id)} ({member_id})"
                                            }
                                        }
                                    }
                                }

                                // Action buttons
                                div { class: "flex gap-3",
                                    button {
                                        class: "px-4 py-2 bg-surface-hover hover:bg-border text-text rounded-lg transition-colors",
                                        onclick: move |_| {
                                            show_add_form.set(false);
                                            selected_member.set(None);
                                        },
                                        "Cancel"
                                    }
                                    button {
                                        class: "px-4 py-2 bg-accent hover:bg-accent-hover text-white rounded-lg transition-colors disabled:opacity-50 disabled:cursor-not-allowed",
                                        disabled: selected_member.read().is_none(),
                                        onclick: add_admin,
                                        "Add as Admin"
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                div { class: "border-t border-border pt-6",
                    p { class: "text-text-muted text-sm",
                        "Only the board owner can add admins."
                    }
                }
            }
        }
    }
}
