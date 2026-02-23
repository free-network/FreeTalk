use crate::components::app::{Route, CURRENT_ROOM, MEMBER_INFO_MODAL, ROOMS};
use crate::components::conversation::message_input::PostInput;
use crate::util::avatar::get_avatar;
use crate::util::ecies::unseal_bytes_with_secrets;
use crate::util::markdown::text_to_html;
use crate::util::messaging::{send_message, ReplyContext};
use crate::util::{format_utc_as_full_datetime, format_utc_as_local_time};
use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use river_core::room_state::content::{TextContentV1, CONTENT_TYPE_TEXT};
use river_core::room_state::member::MemberId;
use river_core::room_state::member_info::MemberInfoV1;
use river_core::room_state::message::{MessagesV1, RoomMessageBody};
use river_core::room_state::privacy::PrivacyMode;
use std::collections::HashMap;

/// A single post for display (text messages only, no replies)
#[derive(Clone, PartialEq)]
struct Post {
    author_id: MemberId,
    author_name: String,
    title: String,
    content_text: String,
    content_html: String,
    time: DateTime<Utc>,
    time_clamped: bool,
    id: String,
}

/// Extract only text posts (no replies, no actions)
fn get_posts(
    messages_state: &MessagesV1,
    member_info: &MemberInfoV1,
    secrets: &HashMap<u32, [u8; 32]>,
) -> Vec<Post> {
    let mut posts = Vec::new();

    for message in messages_state.display_messages() {
        // Only include text messages (not replies or actions)
        if !is_text_message(&message.message.content) {
            continue;
        }

        let author_id = message.message.author;
        let now = Utc::now();
        let raw_time = DateTime::<Utc>::from(message.message.time);
        let time_clamped = raw_time > now;
        let time = if time_clamped { now } else { raw_time };
        let message_id = message.id();

        let author_name = member_info
            .member_info
            .iter()
            .find(|ami| ami.member_info.member_id == author_id)
            .map(|ami| {
                match unseal_bytes_with_secrets(&ami.member_info.preferred_nickname, secrets) {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                    Err(_) => ami.member_info.preferred_nickname.to_string_lossy(),
                }
            })
            .unwrap_or_else(|| "Unknown".to_string());

        let (title, content_text) = decrypt_text_content(&message.message.content, secrets);
        let content_html = text_to_html(&content_text);

        posts.push(Post {
            author_id,
            author_name,
            title,
            content_text,
            content_html,
            time,
            time_clamped,
            id: format!("{:?}", message_id.0),
        });
    }

    posts
}

/// Check if the message is a plain text message (not a reply or action)
fn is_text_message(content: &RoomMessageBody) -> bool {
    match content {
        RoomMessageBody::Public { content_type, .. } => *content_type == CONTENT_TYPE_TEXT,
        RoomMessageBody::Private { content_type, .. } => *content_type == CONTENT_TYPE_TEXT,
    }
}

/// Decrypt text content and return (title, content)
fn decrypt_text_content(
    content: &RoomMessageBody,
    secrets: &HashMap<u32, [u8; 32]>,
) -> (String, String) {
    match content {
        RoomMessageBody::Public {
            content_type, data, ..
        } => {
            if *content_type == CONTENT_TYPE_TEXT {
                if let Ok(text_content) = TextContentV1::decode(data) {
                    return (text_content.title, text_content.content);
                }
            }
            (String::new(), String::new())
        }
        RoomMessageBody::Private {
            content_type,
            ciphertext,
            nonce,
            secret_version,
            ..
        } => {
            if let Some(secret) = secrets.get(secret_version) {
                use crate::util::ecies::decrypt_with_symmetric_key;
                if let Ok(decrypted_bytes) =
                    decrypt_with_symmetric_key(secret, ciphertext.as_slice(), nonce)
                {
                    if *content_type == CONTENT_TYPE_TEXT {
                        if let Ok(text_content) = TextContentV1::decode(&decrypted_bytes) {
                            return (text_content.title, text_content.content);
                        }
                    }
                }
            }
            (String::new(), "[Encrypted]".to_string())
        }
    }
}

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

    // Get room name
    let current_room_label = use_memo({
        move || {
            let current_room = CURRENT_ROOM.read();
            if let Some(key) = current_room.owner_key {
                let rooms = ROOMS.read();
                if let Some(room_data) = rooms.map.get(&key) {
                    let sealed_name = &room_data
                        .room_state
                        .configuration
                        .configuration
                        .display
                        .name;
                    return match unseal_bytes_with_secrets(sealed_name, &room_data.secrets) {
                        Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                        Err(_) => sealed_name.to_string_lossy(),
                    };
                }
            }
            "No Room Selected".to_string()
        }
    });

    // Get posts (memoized)
    let posts = use_memo(move || {
        let current_room = CURRENT_ROOM.read();
        if let Some(key) = current_room.owner_key {
            let rooms = ROOMS.read();
            if let Some(room_data) = rooms.map.get(&key) {
                return Some(get_posts(
                    &room_data.room_state.recent_messages,
                    &room_data.room_state.member_info,
                    &room_data.secrets,
                ));
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

                            // Room name
                            div { class: "px-6 py-2 border-b border-border",
                                h2 { class: "text-lg font-semibold text-text-muted",
                                    "{current_room_label}"
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
                                                let time_str = format_utc_as_local_time(post.time.timestamp_millis());
                                                let full_time_str = format_utc_as_full_datetime(post.time.timestamp_millis());
                                                let author_id = post.author_id;
                                                let post_id = post.id.clone();
                                                rsx! {
                                                    Link {
                                                        key: "{post.id}",
                                                        to: Route::Post { id: post_id },
                                                        class: "block bg-panel rounded-2xl border border-border shadow-sm overflow-hidden hover:border-accent/50 transition-colors",
                                                        // Post header with author
                                                        div { class: "flex items-center gap-4 px-6 py-4 border-b border-border bg-surface/30",
                                                            img {
                                                                src: "{get_avatar(&post.author_id)}",
                                                                alt: "Avatar",
                                                                class: "w-14 h-14 rounded-full"
                                                            }
                                                            div { class: "flex-1 min-w-0",
                                                                div {
                                                                    class: "text-lg font-semibold text-text",
                                                                    onclick: move |e| {
                                                                        e.stop_propagation();
                                                                        MEMBER_INFO_MODAL.with_mut(|signal| {
                                                                            signal.member = Some(author_id);
                                                                        });
                                                                    },
                                                                    "{post.author_name}"
                                                                }
                                                                span {
                                                                    class: if post.time_clamped {
                                                                        "text-sm text-text-muted italic"
                                                                    } else {
                                                                        "text-sm text-text-muted"
                                                                    },
                                                                    title: "{full_time_str}",
                                                                    if post.time_clamped { "~{time_str}" } else { "{time_str}" }
                                                                }
                                                            }
                                                        }
                                                        // Post content
                                                        div { class: "px-6 py-6",
                                                            // Title
                                                            if !post.title.is_empty() {
                                                                h2 { class: "text-2xl font-bold text-text mb-4",
                                                                    "{post.title}"
                                                                }
                                                            }
                                                            // Content
                                                            div { class: "text-lg text-text leading-relaxed",
                                                                span {
                                                                    class: "prose prose-lg dark:prose-invert max-w-none",
                                                                    dangerous_inner_html: "{post.content_html}"
                                                                }
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

                // Post input (compose button and modal)
                PostInput {
                    handle_send_message: move |msg: (String, String, Option<ReplyContext>)| {
                        handle_send_message(msg)
                    },
                    replying_to: replying_to,
                    on_request_edit_last: move |_| {},
                }
            }
        }
    }
}

/// Single post view - displays a single post by ID
#[component]
pub fn SinglePostView(post_id: String) -> Element {
    // Find the post by ID
    let post = use_memo(move || {
        let current_room = CURRENT_ROOM.read();
        if let Some(key) = current_room.owner_key {
            let rooms = ROOMS.read();
            if let Some(room_data) = rooms.map.get(&key) {
                let posts = get_posts(
                    &room_data.room_state.recent_messages,
                    &room_data.room_state.member_info,
                    &room_data.secrets,
                );
                return posts.into_iter().find(|p| p.id == post_id);
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

            // Post content
            div { class: "flex-1 overflow-y-auto",
                div { class: "max-w-4xl mx-auto px-4 py-6",
                    {
                        match post.read().as_ref() {
                            Some(post) => {
                                let time_str = format_utc_as_local_time(post.time.timestamp_millis());
                                let full_time_str = format_utc_as_full_datetime(post.time.timestamp_millis());
                                let author_id = post.author_id;
                                rsx! {
                                    div { class: "bg-panel rounded-2xl border border-border shadow-sm overflow-hidden",
                                        // Post header with author
                                        div { class: "flex items-center gap-4 px-8 py-6 border-b border-border bg-surface/30",
                                            img {
                                                src: "{get_avatar(&post.author_id)}",
                                                alt: "Avatar",
                                                class: "w-16 h-16 rounded-full"
                                            }
                                            div { class: "flex-1 min-w-0",
                                                div {
                                                    class: "text-xl font-semibold text-text cursor-pointer hover:text-accent transition-colors",
                                                    onclick: move |_| {
                                                        MEMBER_INFO_MODAL.with_mut(|signal| {
                                                            signal.member = Some(author_id);
                                                        });
                                                    },
                                                    "{post.author_name}"
                                                }
                                                span {
                                                    class: if post.time_clamped {
                                                        "text-sm text-text-muted italic"
                                                    } else {
                                                        "text-sm text-text-muted"
                                                    },
                                                    title: "{full_time_str}",
                                                    if post.time_clamped { "~{time_str}" } else { "{time_str}" }
                                                }
                                            }
                                        }
                                        // Post content - larger for single view
                                        div { class: "px-8 py-8",
                                            // Title
                                            if !post.title.is_empty() {
                                                h1 { class: "text-3xl font-bold text-text mb-6",
                                                    "{post.title}"
                                                }
                                            }
                                            // Content
                                            div { class: "text-xl text-text leading-relaxed",
                                                span {
                                                    class: "prose prose-xl dark:prose-invert max-w-none",
                                                    dangerous_inner_html: "{post.content_html}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            None => {
                                rsx! {
                                    div { class: "flex flex-col items-center justify-center h-64 text-text-muted",
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
}
