//! Admin management view for viewing and adding admins.

mod add_admin_modal;

use add_admin_modal::AddAdminModal;

use crate::board_data::BoardData;
use crate::components::app::{BOARDS, CURRENT_BOARD, EDIT_BOARD_MODAL};
use dioxus::prelude::*;
use river_core::board_state::member::MemberId;

#[component]
pub fn AdminView() -> Element {
    let current_board_data: Memo<Option<BoardData>> = use_memo(move || {
        CURRENT_BOARD
            .read()
            .owner_key
            .as_ref()
            .and_then(|key| BOARDS.read().map.get(key).cloned())
    });

    let mut show_add_admin_modal = use_signal(|| false);

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

            // Action buttons row (owner only)
            if *is_owner.read() {
                div { class: "flex gap-3 mb-6",
                    button {
                        class: "px-4 py-2 bg-surface hover:bg-surface-hover text-text rounded-lg transition-colors flex items-center gap-2",
                        onclick: move |_| {
                            if let Some(owner_key) = CURRENT_BOARD.read().owner_key {
                                EDIT_BOARD_MODAL.write().board = Some(owner_key);
                            }
                        },
                        span { "⚙" }
                        span { "Board Settings" }
                    }
                    button {
                        class: "px-4 py-2 bg-accent hover:bg-accent-hover text-white rounded-lg transition-colors flex items-center gap-2",
                        onclick: move |_| show_add_admin_modal.set(true),
                        span { "+" }
                        span { "Add Admin" }
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

            // Non-owner message
            if !*is_owner.read() {
                div { class: "bg-surface rounded-lg p-4",
                    p { class: "text-text-muted text-sm",
                        "Only the board owner can add admins and modify board settings."
                    }
                }
            }
        }

        // Add Admin Modal
        if *show_add_admin_modal.read() {
            AddAdminModal {
                on_close: move |_| show_add_admin_modal.set(false)
            }
        }
    }
}
