use crate::components::app::{Route, CURRENT_ROOM, MEMBER_INFO_MODAL, ROOMS};
use crate::components::conversation::message_input::PostInput;
use crate::components::conversation::{get_all_messages, get_top_level_posts, PostCard};
use crate::room_data::SendMessageError;
use crate::util::avatar::get_avatar;
use crate::util::ecies::unseal_bytes_with_secrets;
use crate::util::messaging::{send_message, ReplyContext};
use dioxus::prelude::*;
use river_core::room_state::member::MemberId;
use river_core::room_state::privacy::PrivacyMode;

#[component]
pub fn PostsView() -> Element {
    let replying_to = use_signal(|| None::<ReplyContext>);

    let current_room_data = {
        let current_room = CURRENT_ROOM.read();
        if let Some(key) = current_room.owner_key {
            let rooms = ROOMS.read();
            rooms.map.get(&key).cloned()
        } else {
            None
        }
    };

    let has_room_selected = current_room_data.is_some();

    // Get top-level posts (memoized)
    let posts = use_memo(move || {
        let current_room = CURRENT_ROOM.read();
        if let Some(key) = current_room.owner_key {
            let rooms = ROOMS.read();
            if let Some(room_data) = rooms.map.get(&key) {
                let self_member_id = MemberId::from(&room_data.self_sk.verifying_key());
                let all_messages = get_all_messages(
                    &room_data.room_state.recent_messages,
                    &room_data.room_state.member_info,
                    self_member_id,
                    &room_data.secrets,
                );
                return Some(get_top_level_posts(&all_messages));
            }
        }
        None
    });

    // Message sending handler
    let handle_send_message =
        move |(title_text, message_text, reply_ctx): (String, String, Option<ReplyContext>)| {
            if message_text.is_empty() {
                return;
            }

            // Get room data for sending
            let room_info = {
                let current_room = CURRENT_ROOM.read();
                if let Some(key) = current_room.owner_key {
                    let rooms = ROOMS.read();
                    if let Some(room_data) = rooms.map.get(&key) {
                        let is_private = room_data
                            .room_state
                            .configuration
                            .configuration
                            .privacy_mode
                            == PrivacyMode::Private;
                        let secret_opt = if is_private {
                            room_data
                                .secrets
                                .iter()
                                .max_by_key(|(v, _)| *v)
                                .map(|(v, s)| (*s, *v))
                        } else {
                            None
                        };
                        Some((
                            key,
                            room_data.room_key(),
                            room_data.self_sk.clone(),
                            room_data.room_state.clone(),
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

            if let Some((current_room, room_key, self_sk, room_state_clone, is_private, secret_opt)) =
                room_info
            {
                spawn(async move {
                    send_message(
                        current_room,
                        room_key,
                        self_sk,
                        room_state_clone,
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

    rsx! {
        div { class: "flex-1 flex flex-col min-w-0 bg-bg",
            // Show no-board-selected message or header with user info
            if !has_room_selected {
                div { class: "flex-1 flex flex-col items-center justify-center text-text-muted",
                    p { class: "text-xl", "Select a board from the sidebar above or create one" }
                }
            } else {
                {
                    current_room_data.as_ref().map(|room_data| {
                        let self_member_id = MemberId::from(&room_data.self_sk.verifying_key());
                        let self_nickname = room_data.room_state.member_info.member_info
                            .iter()
                            .find(|ami| ami.member_info.member_id == self_member_id)
                            .map(|ami| {
                                match unseal_bytes_with_secrets(&ami.member_info.preferred_nickname, &room_data.secrets) {
                                    Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                                    Err(_) => ami.member_info.preferred_nickname.to_string_lossy(),
                                }
                            })
                            .unwrap_or_else(|| "You".to_string());
                        let self_avatar = get_avatar(&self_member_id);
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
                                }
                                div {
                                    match room_data.can_participate() {
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
                                            div { class: "px-4 py-3 mx-4 mb-4 bg-error-bg text-red-700 dark:text-red-400 rounded-lg text-sm",
                                                "You have been banned from sending messages in this room."
                                            }
                                        },
                                    }
                                }
                            }
                        }
                    })
                }

                // Posts list
                div { class: "flex-1 overflow-y-auto",
                    div { class: "max-w-4xl mx-auto px-4 py-6",
                        {
                            match posts.read().as_ref() {
                                Some(posts) if !posts.is_empty() => {
                                    rsx! {
                                        div { class: "space-y-8",
                                            {posts.iter().map(|post| {
                                                let post_id = post.id_string();
                                                let nav = navigator();
                                                rsx! {
                                                    div { key: "{post_id}",
                                                        PostCard {
                                                            message: post.clone(),
                                                            expanded: false,
                                                            show_replies: false,
                                                            on_click: move |_| {
                                                                nav.push(Route::Post { id: post_id.clone() });
                                                            },
                                                        }
                                                    }
                                                }
                                            })}
                                        }
                                    }
                                }
                                _ => {
                                    rsx! {
                                        div { class: "flex flex-col items-center justify-center h-64 text-text-muted",
                                            p { class: "text-xl", "No posts yet." }
                                            p { class: "text-sm mt-2", "Be the first to share something!" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Single post view - displays a single post by ID with replies
#[component]
pub fn SinglePostView(post_id: String) -> Element {
    // Find the post by ID
    let post_data = use_memo(move || {
        let current_room = CURRENT_ROOM.read();
        if let Some(key) = current_room.owner_key {
            let rooms = ROOMS.read();
            if let Some(room_data) = rooms.map.get(&key) {
                let self_member_id = MemberId::from(&room_data.self_sk.verifying_key());
                let all_messages = get_all_messages(
                    &room_data.room_state.recent_messages,
                    &room_data.room_state.member_info,
                    self_member_id,
                    &room_data.secrets,
                );
                return all_messages
                    .into_iter()
                    .find(|m| m.id_string() == post_id);
            }
        }
        None
    });

    rsx! {
        div { class: "flex-1 flex flex-col min-w-0 bg-bg",
            // Back button header
            div { class: "flex items-center gap-4 px-6 py-4 border-b border-border",
                Link {
                    to: Route::Posts,
                    class: "text-accent hover:text-accent/80 transition-colors text-lg",
                    "← Back to posts"
                }
            }

            // Post content and replies
            div { class: "flex-1 overflow-y-auto",
                {
                    match post_data.read().as_ref() {
                        Some(post) => {
                            rsx! {
                                div { class: "max-w-4xl mx-auto px-4 py-6",
                                    PostCard {
                                        message: post.clone(),
                                        expanded: true,
                                        show_replies: true,
                                    }
                                }
                            }
                        }
                        None => {
                            rsx! {
                                div { class: "flex-1 flex flex-col items-center justify-center h-64 text-text-muted",
                                    p { class: "text-xl", "Post not found." }
                                    Link {
                                        to: Route::Posts,
                                        class: "mt-4 text-accent hover:text-accent/80 transition-colors",
                                        "← Back to posts"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
