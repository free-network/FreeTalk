//! Top bar component showing user profile, admin controls, and post input.

use crate::board_data::SendMessageError;
use crate::components::app::{BOARDS, CURRENT_BOARD, MEMBER_INFO_MODAL};
use crate::components::conversation::message_input::PostInput;
use crate::util::avatar::get_avatar;
use crate::util::ecies::unseal_bytes_with_secrets;
use crate::util::messaging::{send_message, ReplyContext};
use dioxus::prelude::*;
use river_core::board_state::member::MemberId;
use river_core::board_state::privacy::PrivacyMode;

/// Top bar component displaying the current user's profile, admin controls, and post input.
#[component]
pub fn TopBar() -> Element {
    let replying_to = use_signal(|| None::<ReplyContext>);

    // Get current board data
    let current_board_data = use_memo(move || {
        CURRENT_BOARD
            .read()
            .owner_key
            .and_then(|key| BOARDS.read().map.get(&key).cloned())
    });

    // Message sending handler
    let handle_send_message =
        move |(title_text, message_text, reply_ctx): (String, String, Option<ReplyContext>)| {
            if message_text.is_empty() {
                return;
            }

            // Get board data for sending
            let board_info = {
                let current_board = CURRENT_BOARD.read();
                if let Some(key) = current_board.owner_key {
                    let boards = BOARDS.read();
                    if let Some(board_data) = boards.map.get(&key) {
                        let is_private = board_data
                            .board_state
                            .configuration
                            .configuration
                            .privacy_mode
                            == PrivacyMode::Private;
                        let secret_opt = if is_private {
                            board_data
                                .secrets
                                .iter()
                                .max_by_key(|(v, _)| *v)
                                .map(|(v, s)| (*s, *v))
                        } else {
                            None
                        };
                        Some((
                            key,
                            board_data.board_key(),
                            board_data.self_sk.clone(),
                            board_data.board_state.clone(),
                            is_private,
                            secret_opt,
                        ))
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some((
                current_board,
                board_key,
                self_sk,
                board_state_clone,
                is_private,
                secret_opt,
            )) = board_info
            {
                spawn(async move {
                    send_message(
                        current_board,
                        board_key,
                        self_sk,
                        board_state_clone,
                        is_private,
                        secret_opt,
                        title_text,
                        message_text,
                        reply_ctx,
                    )
                    .await;
                });
            }
        };

    // Don't render if no board is selected
    let Some(board_data) = current_board_data.read().clone() else {
        return rsx! {};
    };

    let self_member_id = MemberId::from(&board_data.self_sk.verifying_key());
    let owner_id = MemberId::from(&board_data.owner_vk);
    let is_owner = self_member_id == owner_id;
    let is_admin = board_data
        .board_state
        .admin
        .admins
        .iter()
        .any(|a| a.admin.id() == self_member_id);
    let can_participate = board_data.can_participate();

    let self_nickname = board_data
        .board_state
        .member_info
        .member_info
        .iter()
        .find(|ami| ami.member_info.member_id == self_member_id)
        .map(|ami| {
            match unseal_bytes_with_secrets(
                &ami.member_info.preferred_nickname,
                &board_data.secrets,
            ) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                Err(_) => ami.member_info.preferred_nickname.to_string_lossy(),
            }
        })
        .unwrap_or_else(|| "You".to_string());

    let self_avatar = get_avatar(&self_member_id);
    let board_id = bs58::encode(board_data.owner_vk.as_bytes()).into_string();

    rsx! {
        div { class: "flex justify-between",
            // User profile header
            div {
                class: "flex items-center gap-3 px-6 py-4 cursor-pointer hover:bg-surface/50 transition-colors",
                onclick: move |_| {
                    MEMBER_INFO_MODAL.with_mut(|signal| {
                        signal.member = Some(self_member_id);
                    });
                },
                img {
                    src: "{self_avatar}",
                    alt: "Your avatar",
                    class: "w-16 h-16 rounded-full"
                }
                span { class: "text-3xl font-medium text-text",
                    "{self_nickname}"
                }
                if is_owner {
                    span { class: "text-lg", title: "Board Owner", "👑" }
                } else if is_admin {
                    span { class: "text-lg", title: "Admin", "👑" }
                }
            }

            // Right side: admin button and post input
            div { class: "flex items-center gap-3 pr-4",
                // Admin button for owners or admins
                if is_owner || is_admin {
                    a {
                        href: "#/board/{board_id}/admin",
                        class: "flex items-center gap-2 px-4 py-2 bg-surface hover:bg-surface-hover text-text rounded-lg transition-colors",
                        title: "Manage Admins",
                        span { "⚙" }
                        span { "Admin" }
                    }
                }

                // Post input
                match can_participate {
                    Ok(()) => rsx! {
                        PostInput {
                            handle_send_message: move |msg: (String, String, Option<ReplyContext>)| {
                                handle_send_message(msg)
                            },
                            replying_to: replying_to,
                            on_request_edit_last: move |_| {},
                        }
                    },
                    Err(SendMessageError::UserNotMember) => rsx! {},
                    Err(SendMessageError::UserBanned) => rsx! {
                        div { class: "px-4 py-2 bg-error-bg text-red-700 dark:text-red-400 rounded-lg text-sm",
                            "You have been banned from this board."
                        }
                    },
                }
            }
        }
    }
}
