use crate::components::app::{CURRENT_ROOM, MEMBER_INFO_MODAL, ROOMS};
use crate::util::avatar::get_avatar;
use crate::util::ecies::unseal_bytes_with_secrets;
use crate::util::{format_utc_as_full_datetime, format_utc_as_local_time};
use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use river_core::room_state::content::{TextContentV1, CONTENT_TYPE_TEXT};
use river_core::room_state::member::MemberId;
use river_core::room_state::member_info::MemberInfoV1;
use river_core::room_state::message::{AuthorizedMessageV1, MessagesV1, RoomMessageBody};
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

/// Convert text to HTML with clickable links
fn text_to_html(text: &str) -> String {
    let linkified = auto_linkify_urls(text);
    let with_hard_breaks = linkified.replace("\n", "  \n");
    let html = markdown::to_html(&with_hard_breaks);
    make_links_open_in_new_tab(&html)
}

/// Auto-linkify plain URLs
fn auto_linkify_urls(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        if c == ']' {
            result.push(c);
            if let Some(&(_, '(')) = chars.peek() {
                result.push(chars.next().unwrap().1);
                for (_, ch) in chars.by_ref() {
                    result.push(ch);
                    if ch == ')' {
                        break;
                    }
                }
            }
            continue;
        }

        let remaining = &text[i..];
        if remaining.starts_with("http://") || remaining.starts_with("https://") {
            let before = &text[..i];
            let is_in_markdown_link = {
                let mut depth = 0i32;
                let mut in_link_url = false;
                for ch in before.chars().rev() {
                    if ch == ')' {
                        depth += 1;
                    } else if ch == '(' {
                        if depth > 0 {
                            depth -= 1;
                        } else {
                            in_link_url = true;
                            break;
                        }
                    } else if ch == ']' && depth == 0 {
                        break;
                    }
                }
                in_link_url
            };

            if is_in_markdown_link {
                result.push(c);
                continue;
            }

            let url_end = remaining
                .find(|ch: char| ch.is_whitespace() || ch == '<' || ch == '>' || ch == '"')
                .unwrap_or(remaining.len());

            let mut url = &remaining[..url_end];
            while url.ends_with(['.', ',', ';', ':', '!', '?', ')', ']']) {
                url = &url[..url.len() - 1];
            }

            result.push('[');
            result.push_str(url);
            result.push_str("](");
            result.push_str(url);
            result.push(')');

            for _ in 0..url.len() - 1 {
                chars.next();
            }
        } else {
            result.push(c);
        }
    }

    result
}

fn make_links_open_in_new_tab(html: &str) -> String {
    html.replace(
        "<a href=\"",
        "<a target=\"_blank\" rel=\"noopener noreferrer\" href=\"",
    )
}

#[component]
pub fn PostsView() -> Element {
    let current_room_data = {
        let current_room = CURRENT_ROOM.read();
        if let Some(key) = current_room.owner_key {
            let rooms = ROOMS.read();
            rooms.map.get(&key).cloned()
        } else {
            None
        }
    };

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

    rsx! {
        div { class: "flex-1 flex flex-col min-w-0 bg-bg",
            // Header with user info
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
                                            rsx! {
                                                div {
                                                    key: "{post.id}",
                                                    class: "bg-panel rounded-2xl border border-border shadow-sm overflow-hidden",
                                                    // Post header with author
                                                    div { class: "flex items-center gap-4 px-6 py-4 border-b border-border bg-surface/30",
                                                        img {
                                                            src: "{get_avatar(&post.author_id)}",
                                                            alt: "Avatar",
                                                            class: "w-14 h-14 rounded-full"
                                                        }
                                                        div { class: "flex-1 min-w-0",
                                                            div {
                                                                class: "text-lg font-semibold text-text cursor-pointer hover:text-accent transition-colors",
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
        }
    }
}
