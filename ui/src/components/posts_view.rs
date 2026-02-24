use crate::components::app::{Route, CURRENT_ROOM, MEMBER_INFO_MODAL, ROOMS};
use crate::components::conversation::message_input::PostInput;
use crate::components::conversation::{get_all_messages, get_top_level_posts, MessageCard, MessageCardVariant};
use crate::room_data::SendMessageError;
use crate::util::avatar::get_avatar;
use crate::util::ecies::unseal_bytes_with_secrets;
use crate::util::message_actions::{self, ActionContext};
use crate::util::messaging::{send_message, ReplyContext};
use dioxus::prelude::*;
use river_core::room_state::member::MemberId;
use river_core::room_state::message::MessageId;
use river_core::room_state::privacy::PrivacyMode;
use wasm_bindgen_futures::spawn_local;

#[component]
pub fn PostsView() -> Element {
    let mut replying_to = use_signal(|| None::<ReplyContext>);
    let mut pending_delete: Signal<Option<MessageId>> = use_signal(|| None);

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

    // Handler for toggling reactions
    let handle_toggle_reaction = move |target_message_id: MessageId, emoji: String| {
        if let Some(ctx) = ActionContext::from_current_room() {
            spawn_local(async move {
                message_actions::toggle_reaction(ctx, target_message_id, emoji).await;
            });
        }
    };

    // Handler for deleting messages
    let handle_delete_message = move |target_message_id: MessageId| {
        if let Some(ctx) = ActionContext::from_current_room() {
            spawn_local(async move {
                message_actions::delete_message(ctx, target_message_id).await;
            });
        }
    };

    // Handler for editing messages
    let handle_edit_message = move |target_message_id: MessageId, new_title: String, new_text: String| {
        if let Some(ctx) = ActionContext::from_current_room() {
            spawn_local(async move {
                message_actions::edit_message(ctx, target_message_id, new_title, new_text).await;
            });
        }
    };

    rsx! {
        div { class: "flex-1 flex flex-col min-w-0 bg-bg",
            // Show no-board-selected message or header with user info
            if !has_room_selected {
                div { class: "flex flex-col items-center justify-center h-64 text-text-muted",
                    p { class: "text-xl", "Select a board from the sidebar above or create one" }
                    p { class: "text-sm mt-2", "Posts will appear here" }
                }
            } else {
                {
                    current_room_data.as_ref().map(|room_data| {
                        let self_member_id = MemberId::from(&room_data.self_sk.verifying_key());
                        let owner_id = MemberId::from(&room_data.owner_vk);
                        let is_owner = self_member_id == owner_id;
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
                        let room_id = bs58::encode(room_data.owner_vk.as_bytes()).into_string();
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
                                        span { class: "text-2xl", title: "Board Owner", "👑" }
                                    }
                                }
                                // Admin button for owners
                                if is_owner {
                                    a {
                                        href: "#/room/{room_id}/admin",
                                        class: "flex items-center gap-2 px-4 py-2 mr-4 bg-surface hover:bg-surface-hover text-text rounded-lg transition-colors self-center",
                                        title: "Manage Admins",
                                        span { "⚙" }
                                        span { "Admin" }
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
                            let self_member_id_for_posts = current_room_data.as_ref()
                                .map(|rd| MemberId::from(&rd.self_sk.verifying_key()));
                            match (posts.read().as_ref(), self_member_id_for_posts) {
                                (Some(posts), Some(self_member_id)) if !posts.is_empty() => {
                                    rsx! {
                                        div { class: "space-y-8",
                                            {posts.iter().map({
                                                let handle_toggle_reaction = handle_toggle_reaction.clone();
                                                let handle_edit_message = handle_edit_message.clone();
                                                move |post| {
                                                    let post_id = post.id_string();
                                                    let nav = navigator();
                                                    let handle_toggle_reaction = handle_toggle_reaction.clone();
                                                    let handle_edit_message = handle_edit_message.clone();
                                                    rsx! {
                                                        div { key: "{post_id}",
                                                            MessageCard {
                                                                message: post.clone(),
                                                                variant: MessageCardVariant::Card,
                                                                self_member_id: self_member_id,
                                                                expanded: false,
                                                                show_replies: false,
                                                                on_click: move |_| {
                                                                    // Get current room_id for navigation
                                                                    if let Some(key) = CURRENT_ROOM.read().owner_key {
                                                                        let room_id = bs58::encode(key.as_bytes()).into_string();
                                                                        nav.push(Route::Post { room_id, post_id: post_id.clone() });
                                                                    }
                                                                },
                                                                on_react: move |(msg_id, emoji)| {
                                                                    handle_toggle_reaction(msg_id, emoji);
                                                                },
                                                                on_request_delete: move |msg_id| {
                                                                    pending_delete.set(Some(msg_id));
                                                                },
                                                                on_edit: move |(msg_id, new_title, new_text)| {
                                                                    handle_edit_message(msg_id, new_title, new_text);
                                                                },
                                                            }
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

                // Delete confirmation modal
                if pending_delete.read().is_some() {
                    div {
                        class: "fixed inset-0 bg-black/50 flex items-center justify-center z-50",
                        onclick: move |_| pending_delete.set(None),
                        div {
                            class: "bg-panel rounded-lg shadow-xl p-6 max-w-sm mx-4",
                            onclick: move |e| e.stop_propagation(),
                            h3 { class: "text-lg font-semibold text-text mb-2",
                                "Delete Post?"
                            }
                            p { class: "text-text-muted text-sm mb-4",
                                "This action cannot be undone."
                            }
                            div { class: "flex gap-3 justify-end",
                                button {
                                    class: "px-4 py-2 rounded-lg bg-surface hover:bg-surface/80 text-text transition-colors",
                                    onclick: move |_| pending_delete.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "px-4 py-2 rounded-lg bg-red-500 hover:bg-red-600 text-white transition-colors",
                                    onclick: move |_| {
                                        let msg_id_opt = pending_delete.read().clone();
                                        if let Some(msg_id) = msg_id_opt {
                                            handle_delete_message(msg_id);
                                        }
                                        pending_delete.set(None);
                                    },
                                    "Delete"
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
    let mut pending_delete: Signal<Option<MessageId>> = use_signal(|| None);

    // Get current room_id for navigation
    let current_room_id = use_memo(move || {
        CURRENT_ROOM
            .read()
            .owner_key
            .map(|key| bs58::encode(key.as_bytes()).into_string())
            .unwrap_or_default()
    });

    // Find the post and self_member_id
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
                let post = all_messages
                    .into_iter()
                    .find(|m| m.id_string() == post_id);
                return post.map(|p| (p, self_member_id));
            }
        }
        None
    });

    // Handler for toggling reactions
    let handle_toggle_reaction = move |target_message_id: MessageId, emoji: String| {
        if let Some(ctx) = ActionContext::from_current_room() {
            spawn_local(async move {
                message_actions::toggle_reaction(ctx, target_message_id, emoji).await;
            });
        }
    };

    // Handler for deleting messages
    let handle_delete_message = move |target_message_id: MessageId| {
        if let Some(ctx) = ActionContext::from_current_room() {
            spawn_local(async move {
                message_actions::delete_message(ctx, target_message_id).await;
            });
        }
    };

    // Handler for editing messages
    let handle_edit_message = move |target_message_id: MessageId, new_title: String, new_text: String| {
        if let Some(ctx) = ActionContext::from_current_room() {
            spawn_local(async move {
                message_actions::edit_message(ctx, target_message_id, new_title, new_text).await;
            });
        }
    };

    rsx! {
        div { class: "flex-1 flex flex-col min-w-0 bg-bg",
            // Back button header
            div { class: "flex items-center gap-4 px-6 py-4 border-b border-border",
                Link {
                    to: Route::Posts { room_id: current_room_id.read().clone() },
                    class: "text-accent hover:text-accent/80 transition-colors text-lg",
                    "← Back to posts"
                }
            }

            // Post content and replies
            div { class: "flex-1 overflow-y-auto",
                {
                    match post_data.read().as_ref() {
                        Some((post, self_member_id)) => {
                            rsx! {
                                div { class: "max-w-4xl mx-auto px-4 py-6",
                                    MessageCard {
                                        message: post.clone(),
                                        variant: MessageCardVariant::Card,
                                        self_member_id: *self_member_id,
                                        expanded: true,
                                        show_replies: true,
                                        on_react: move |(msg_id, emoji)| {
                                            handle_toggle_reaction(msg_id, emoji);
                                        },
                                        on_request_delete: move |msg_id| {
                                            pending_delete.set(Some(msg_id));
                                        },
                                        on_edit: move |(msg_id, new_title, new_text)| {
                                            handle_edit_message(msg_id, new_title, new_text);
                                        },
                                    }
                                }
                            }
                        }
                        None => {
                            rsx! {
                                div { class: "flex-1 flex flex-col items-center justify-center h-64 text-text-muted",
                                    p { class: "text-xl", "Post not found." }
                                    Link {
                                        to: Route::Posts { room_id: current_room_id.read().clone() },
                                        class: "mt-4 text-accent hover:text-accent/80 transition-colors",
                                        "← Back to posts"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Delete confirmation modal
            if pending_delete.read().is_some() {
                div {
                    class: "fixed inset-0 bg-black/50 flex items-center justify-center z-50",
                    onclick: move |_| pending_delete.set(None),
                    div {
                        class: "bg-panel rounded-lg shadow-xl p-6 max-w-sm mx-4",
                        onclick: move |e| e.stop_propagation(),
                        h3 { class: "text-lg font-semibold text-text mb-2",
                            "Delete Post?"
                        }
                        p { class: "text-text-muted text-sm mb-4",
                            "This action cannot be undone."
                        }
                        div { class: "flex gap-3 justify-end",
                            button {
                                class: "px-4 py-2 rounded-lg bg-surface hover:bg-surface/80 text-text transition-colors",
                                onclick: move |_| pending_delete.set(None),
                                "Cancel"
                            }
                            button {
                                class: "px-4 py-2 rounded-lg bg-red-500 hover:bg-red-600 text-white transition-colors",
                                onclick: move |_| {
                                    let msg_id_opt = pending_delete.read().clone();
                                    if let Some(msg_id) = msg_id_opt {
                                        handle_delete_message(msg_id);
                                    }
                                    pending_delete.set(None);
                                },
                                "Delete"
                            }
                        }
                    }
                }
            }
        }
    }
}
