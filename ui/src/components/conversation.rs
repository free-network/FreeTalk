use crate::components::app::receive_times::get_delay_secs;
use crate::components::app::{CURRENT_ROOM, MEMBER_INFO_MODAL, ROOMS};
use crate::util::avatar::get_avatar;
use crate::util::ecies::unseal_bytes_with_secrets;
use crate::util::markdown::text_to_html;
use crate::util::messaging::{send_message, ReplyContext};
use crate::util::{format_utc_as_full_datetime, format_utc_as_local_time};
pub mod emoji_picker;
mod message_actions;
pub mod message_input;
mod not_member_notification;
use self::emoji_picker::FREQUENT_EMOJIS;
use self::not_member_notification::NotMemberNotification;
use crate::components::conversation::message_input::PostInput;
use crate::room_data::SendMessageError;
use chrono::{DateTime, Utc};
use dioxus::prelude::*;
use river_core::room_state::member::MemberId;
use river_core::room_state::member_info::MemberInfoV1;
use river_core::room_state::message::{MessageId, MessagesV1, RoomMessageBody};
use std::collections::HashMap;
use wasm_bindgen_futures::spawn_local;

/// A single message for display
#[derive(Clone, PartialEq, Debug)]
pub struct MessageData {
    pub message_id: MessageId,
    pub author_id: MemberId,
    pub author_name: String,
    pub is_self: bool,
    pub title_text: String,
    pub content_text: String,
    pub content_html: String,
    pub time: DateTime<Utc>,
    pub time_clamped: bool,
    pub edited: bool,
    pub reactions: HashMap<String, Vec<MemberId>>,
    pub reply_to_message_id: Option<MessageId>,
    pub reply_to_author: Option<String>,
    pub reply_to_preview: Option<String>,
    pub receive_delay_secs: Option<i64>,
}

/// A message with its nested replies
#[derive(Clone, PartialEq, Debug)]
pub struct MessageWithReplies {
    pub message: MessageData,
    pub replies: Vec<MessageWithReplies>,
}

impl MessageData {
    /// Check if this message is a top-level post (not a reply to another message)
    pub fn is_top_level_post(&self) -> bool {
        self.reply_to_message_id.is_none()
    }

    /// Get a string ID suitable for use in URLs/routes
    pub fn id_string(&self) -> String {
        format!("{:?}", self.message_id.0)
    }
}

/// Build a flat list of all messages
pub fn get_all_messages(
    messages_state: &MessagesV1,
    member_info: &MemberInfoV1,
    self_member_id: MemberId,
    secrets: &HashMap<u32, [u8; 32]>,
) -> Vec<MessageData> {
    let mut messages = Vec::new();

    for message in messages_state.display_messages() {
        let author_id = message.message.author;
        let now = Utc::now();
        let raw_time = DateTime::<Utc>::from(message.message.time);
        let time_clamped = raw_time > now;
        let message_time = if time_clamped { now } else { raw_time };
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

        let content_text = messages_state
            .effective_text(message)
            .unwrap_or_else(|| decrypt_message_content(&message.message.content, secrets));
        let content_html = text_to_html(&content_text);
        let title_text = decrypt_message_title(&message.message.content, secrets);
        let is_self = author_id == self_member_id;

        let edited = messages_state.is_edited(&message_id);
        let reactions = messages_state
            .reactions(&message_id)
            .cloned()
            .unwrap_or_default();

        let (reply_to_author, reply_to_preview, reply_to_message_id) =
            extract_reply_context(&message.message.content, secrets);

        let send_time_ms = raw_time.timestamp_millis();
        let receive_delay_secs = get_delay_secs(&message_id, send_time_ms);

        messages.push(MessageData {
            message_id,
            author_id,
            author_name,
            is_self,
            title_text,
            content_text,
            content_html,
            time: message_time,
            time_clamped,
            edited,
            reactions,
            reply_to_message_id,
            reply_to_author,
            reply_to_preview,
            receive_delay_secs,
        });
    }

    messages
}

/// Build a tree of messages with their replies
/// If parent_id is None, returns top-level messages (those not replying to anything)
/// If parent_id is Some(id), returns only replies to that specific message
pub fn build_reply_tree(
    all_messages: &[MessageData],
    parent_id: Option<&MessageId>,
) -> Vec<MessageWithReplies> {
    // Find messages that reply to the given parent
    let direct_replies: Vec<&MessageData> = all_messages
        .iter()
        .filter(|m| m.reply_to_message_id.as_ref() == parent_id)
        .collect();

    // Sort by time
    let mut sorted_replies: Vec<_> = direct_replies.into_iter().cloned().collect();
    sorted_replies.sort_by_key(|m| m.time);

    // Recursively build replies for each message
    sorted_replies
        .into_iter()
        .map(|message| {
            let nested_replies = build_reply_tree(all_messages, Some(&message.message_id));
            MessageWithReplies {
                message,
                replies: nested_replies,
            }
        })
        .collect()
}

fn decrypt_message_title(content: &RoomMessageBody, secrets: &HashMap<u32, [u8; 32]>) -> String {
    use river_core::room_state::content::{
        ReplyContentV1, TextContentV1, CONTENT_TYPE_REPLY, CONTENT_TYPE_TEXT,
    };

    match content {
        RoomMessageBody::Public {
            content_type, data, ..
        } => {
            if *content_type == CONTENT_TYPE_TEXT {
                if let Ok(text_content) = TextContentV1::decode(data) {
                    return text_content.title;
                }
            }
            if *content_type == CONTENT_TYPE_REPLY {
                if let Ok(reply) = ReplyContentV1::decode(data) {
                    return reply.title;
                }
            }
            String::new()
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
                            return text_content.title;
                        }
                    }
                    if *content_type == CONTENT_TYPE_REPLY {
                        if let Ok(reply) = ReplyContentV1::decode(&decrypted_bytes) {
                            return reply.title;
                        }
                    }
                }
            }
            String::new()
        }
    }
}

fn decrypt_message_content(content: &RoomMessageBody, secrets: &HashMap<u32, [u8; 32]>) -> String {
    use river_core::room_state::content::{
        ReplyContentV1, TextContentV1, CONTENT_TYPE_ACTION, CONTENT_TYPE_REPLY, CONTENT_TYPE_TEXT,
    };

    match content {
        RoomMessageBody::Public {
            content_type, data, ..
        } => {
            if *content_type == CONTENT_TYPE_ACTION {
                return content.to_string_lossy();
            }
            if *content_type == CONTENT_TYPE_TEXT {
                if let Ok(text_content) = TextContentV1::decode(data) {
                    return text_content.content;
                }
            }
            if *content_type == CONTENT_TYPE_REPLY {
                if let Ok(reply) = ReplyContentV1::decode(data) {
                    return reply.content;
                }
            }
            content.to_string_lossy()
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
                            return text_content.content;
                        }
                    }
                    if *content_type == CONTENT_TYPE_REPLY {
                        if let Ok(reply) = ReplyContentV1::decode(&decrypted_bytes) {
                            return reply.content;
                        }
                    }
                    return String::from_utf8_lossy(&decrypted_bytes).to_string();
                }
                content.to_string_lossy()
            } else {
                format!(
                    "[Encrypted message - secret v{} not available (have: {:?})]",
                    secret_version,
                    secrets.keys().collect::<Vec<_>>()
                )
            }
        }
    }
}

fn extract_reply_context(
    content: &RoomMessageBody,
    secrets: &HashMap<u32, [u8; 32]>,
) -> (Option<String>, Option<String>, Option<MessageId>) {
    use river_core::room_state::content::{ReplyContentV1, CONTENT_TYPE_REPLY};

    match content {
        RoomMessageBody::Public {
            content_type, data, ..
        } if *content_type == CONTENT_TYPE_REPLY => {
            if let Ok(reply) = ReplyContentV1::decode(data) {
                return (
                    Some(reply.target_author_name),
                    Some(reply.target_content_preview),
                    Some(reply.target_message_id),
                );
            }
        }
        RoomMessageBody::Private {
            content_type,
            ciphertext,
            nonce,
            secret_version,
            ..
        } if *content_type == CONTENT_TYPE_REPLY => {
            if let Some(secret) = secrets.get(secret_version) {
                use crate::util::ecies::decrypt_with_symmetric_key;
                if let Ok(decrypted_bytes) =
                    decrypt_with_symmetric_key(secret, ciphertext.as_slice(), nonce)
                {
                    if let Ok(reply) = ReplyContentV1::decode(&decrypted_bytes) {
                        return (
                            Some(reply.target_author_name),
                            Some(reply.target_content_preview),
                            Some(reply.target_message_id),
                        );
                    }
                }
            }
        }
        _ => {}
    }
    (None, None, None)
}

/// Conversation component that shows replies to a specific parent message
/// If parent_message_id is None, this is a standalone conversation (legacy behavior)
/// If parent_message_id is Some(id), shows only replies to that message
#[component]
pub fn Conversation(
    #[props(default)] parent_message_id: Option<MessageId>,
    /// Default reply context - when set, new messages will be replies to this
    #[props(default)] default_reply_to: Option<ReplyContext>,
) -> Element {
    let current_room_data = {
        let current_room = CURRENT_ROOM.read();
        if let Some(key) = current_room.owner_key {
            let rooms = ROOMS.read();
            rooms.map.get(&key).cloned()
        } else {
            None
        }
    };

    let mut replying_to: Signal<Option<ReplyContext>> = use_signal(|| None);
    let mut pending_delete: Signal<Option<MessageId>> = use_signal(|| None);

    // Build message tree for the given parent
    let message_tree = use_memo({
        let parent_id = parent_message_id.clone();
        move || {
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

                    // Build member name lookup
                    let member_names: HashMap<MemberId, String> = room_data
                        .room_state
                        .member_info
                        .member_info
                        .iter()
                        .map(|ami| {
                            let name = match unseal_bytes_with_secrets(
                                &ami.member_info.preferred_nickname,
                                &room_data.secrets,
                            ) {
                                Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                                Err(_) => ami.member_info.preferred_nickname.to_string_lossy(),
                            };
                            (ami.member_info.member_id, name)
                        })
                        .collect();

                    let tree = build_reply_tree(&all_messages, parent_id.as_ref());
                    return Some((tree, self_member_id, member_names));
                }
            }
            None
        }
    });

    // Handler for toggling a reaction
    let handle_toggle_reaction = move |target_message_id: MessageId, emoji: String| {
        if let Some(ctx) = crate::util::message_actions::ActionContext::from_current_room() {
            spawn_local(async move {
                crate::util::message_actions::toggle_reaction(ctx, target_message_id, emoji).await;
            });
        }
    };

    // Handler for deleting a message
    let handle_delete_message = move |target_message_id: MessageId| {
        if let Some(ctx) = crate::util::message_actions::ActionContext::from_current_room() {
            spawn_local(async move {
                crate::util::message_actions::delete_message(ctx, target_message_id).await;
            });
        }
    };

    // Handler for editing a message
    let handle_edit_message = move |target_message_id: MessageId, new_title: String, new_text: String| {
        if let Some(ctx) = crate::util::message_actions::ActionContext::from_current_room() {
            spawn_local(async move {
                crate::util::message_actions::edit_message(ctx, target_message_id, new_title, new_text).await;
            });
        }
    };

    // Message sending handler
    let handle_send_message = {
        let current_room_data = current_room_data.clone();
        move |(title_text, message_text, reply_ctx): (String, String, Option<ReplyContext>)| {
            if let (Some(current_room), Some(current_room_data)) =
                (CURRENT_ROOM.read().owner_key, current_room_data.clone())
            {
                let room_key = current_room_data.room_key();
                let self_sk = current_room_data.self_sk.clone();
                let room_state_clone = current_room_data.room_state.clone();
                let is_private = current_room_data.is_private();
                let secret_opt: Option<([u8; 32], u32)> = current_room_data
                    .get_secret()
                    .map(|(secret, version)| (*secret, version));

                spawn_local(async move {
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
        }
    };

    rsx! {
        div { class: "flex flex-col h-full",
            // Reply thread area
            div { class: "flex-1 overflow-y-auto",
                div { class: "max-w-4xl mx-auto px-4 py-4",
                    {
                        if current_room_data.is_some() {
                            match message_tree.read().as_ref() {
                                Some((tree, self_member_id, member_names)) if !tree.is_empty() => {
                                    let tree = tree.clone();
                                    let self_member_id = *self_member_id;
                                    let member_names = member_names.clone();
                                    Some(rsx! {
                                        div { class: "space-y-2",
                                            {tree.into_iter().map({
                                                let handle_toggle_reaction = handle_toggle_reaction.clone();
                                                let handle_edit_message = handle_edit_message.clone();
                                                let member_names = member_names.clone();
                                                move |msg_with_replies| {
                                                    let handle_toggle_reaction = handle_toggle_reaction.clone();
                                                    let handle_edit_message = handle_edit_message.clone();
                                                    let member_names = member_names.clone();
                                                    rsx! {
                                                        MessageCard {
                                                            message: msg_with_replies.message.clone(),
                                                            variant: MessageCardVariant::Reply,
                                                            self_member_id: self_member_id,
                                                            member_names: member_names,
                                                            replies: msg_with_replies.replies.clone(),
                                                            depth: 0,
                                                            on_react: move |(msg_id, emoji)| {
                                                                handle_toggle_reaction(msg_id, emoji);
                                                            },
                                                            on_request_delete: move |msg_id| {
                                                                pending_delete.set(Some(msg_id));
                                                            },
                                                            on_edit: move |(msg_id, new_title, new_text)| {
                                                                handle_edit_message(msg_id, new_title, new_text);
                                                            },
                                                            on_reply: move |ctx: ReplyContext| {
                                                                replying_to.set(Some(ctx));
                                                            },
                                                        }
                                                    }
                                                }
                                            })}
                                        }
                                    })
                                }
                                Some(_) => Some(rsx! {
                                    div { class: "flex flex-col items-center justify-center h-32 text-text-muted",
                                        p { "No replies yet." }
                                    }
                                }),
                                None => Some(rsx! {
                                    div { class: "flex flex-col items-center justify-center h-32 text-text-muted",
                                        p { "No messages." }
                                    }
                                })
                            }
                        } else {
                            None
                        }
                    }
                }
            }

            // Message input
            {
                match current_room_data.as_ref() {
                    Some(room_data) => {
                        match room_data.can_participate() {
                            Ok(()) => rsx! {
                                div { class: "flex justify-center",
                                    PostInput {
                                        handle_send_message: move |msg: (String, String, Option<ReplyContext>)| {
                                            let handle = handle_send_message.clone();
                                            handle(msg)
                                        },
                                        replying_to: replying_to,
                                        on_request_edit_last: move |_| {},
                                        default_reply_to: default_reply_to.clone(),
                                    }
                                }
                            },
                            Err(SendMessageError::UserNotMember) => {
                                let user_vk = room_data.self_sk.verifying_key();
                                rsx! {
                                    NotMemberNotification {
                                        user_verifying_key: user_vk
                                    }
                                }
                            },
                            Err(SendMessageError::UserBanned) => rsx! {
                                div { class: "px-4 py-3 mx-4 mb-4 bg-error-bg text-red-700 dark:text-red-400 rounded-lg text-sm",
                                    "You have been banned from sending messages in this board."
                                }
                            },
                        }
                    },
                    None => rsx! {},
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
                            "Delete Message?"
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

/// Display variant for MessageCard
#[derive(Clone, Copy, PartialEq, Default)]
pub enum MessageCardVariant {
    /// Reply style: border-left, compact, indented
    #[default]
    Reply,
    /// Card style: full card with header, larger text
    Card,
}

/// Size variant for shared components
#[derive(Clone, Copy, PartialEq, Default)]
pub enum MessageSize {
    /// Compact size for reply style
    #[default]
    Compact,
    /// Normal size for card list view
    Normal,
    /// Large size for expanded card view
    Large,
}

/// Shared header component for messages
#[component]
fn MessageHeader(
    author_id: MemberId,
    author_name: String,
    is_self: bool,
    time_str: String,
    full_time_str: String,
    time_clamped: bool,
    size: MessageSize,
) -> Element {
    let (avatar_size, name_class) = match size {
        MessageSize::Compact => ("w-8 h-8", "text-sm font-medium text-text"),
        MessageSize::Normal => ("w-14 h-14", "text-lg font-semibold text-text"),
        MessageSize::Large => ("w-16 h-16", "text-xl font-semibold text-text"),
    };

    let time_class = match size {
        MessageSize::Compact => if time_clamped { "text-xs text-text-muted italic" } else { "text-xs text-text-muted" },
        _ => if time_clamped { "text-sm text-text-muted italic" } else { "text-sm text-text-muted" },
    };

    rsx! {
        img {
            src: "{get_avatar(&author_id)}",
            alt: "Avatar",
            class: "{avatar_size} rounded-full"
        }
        div { class: if matches!(size, MessageSize::Compact) { "flex items-center gap-3" } else { "flex-1 min-w-0" },
            div {
                class: "{name_class} cursor-pointer hover:text-accent transition-colors",
                onclick: move |e| {
                    e.stop_propagation();
                    MEMBER_INFO_MODAL.with_mut(|signal| {
                        signal.member = Some(author_id);
                    });
                },
                "{author_name}"
            }
            if is_self && matches!(size, MessageSize::Compact) {
                span { class: "text-xs text-accent", "(you)" }
            }
            span {
                class: "{time_class}",
                title: "{full_time_str}",
                if time_clamped && !matches!(size, MessageSize::Compact) { "~{time_str}" } else { "{time_str}" }
            }
        }
    }
}

/// Shared content display component (non-editing mode)
#[component]
fn MessageContentDisplay(
    title_text: String,
    content_html: String,
    edited: bool,
    size: MessageSize,
    expanded: bool,
) -> Element {
    let (title_class, content_class) = match size {
        MessageSize::Compact => (
            "font-semibold text-text mb-1",
            "prose prose-sm dark:prose-invert max-w-none",
        ),
        MessageSize::Normal => (
            "text-2xl font-bold text-text mb-4",
            "prose prose-lg dark:prose-invert max-w-none",
        ),
        MessageSize::Large => (
            "text-3xl font-bold text-text mb-6",
            "prose prose-xl dark:prose-invert max-w-none",
        ),
    };

    let edited_class = if matches!(size, MessageSize::Compact) { "text-xs ml-2 text-text-muted" } else { "text-sm ml-2 text-text-muted" };
    let text_class = if matches!(size, MessageSize::Compact) { "text-sm text-text" } else { "text-lg text-text leading-relaxed" };

    rsx! {
        // Title
        if !title_text.is_empty() {
            match size {
                MessageSize::Large if expanded => rsx! { h1 { class: "{title_class}", "{title_text}" } },
                MessageSize::Compact => rsx! { h4 { class: "{title_class}", "{title_text}" } },
                _ => rsx! { h2 { class: "{title_class}", "{title_text}" } },
            }
        }
        // Content
        div { class: "{text_class}",
            span {
                class: "{content_class}",
                dangerous_inner_html: "{content_html}"
            }
            if edited {
                span { class: "{edited_class}", "(edited)" }
            }
        }
    }
}

/// Shared edit form component
#[component]
fn MessageEditForm(
    edit_title: Signal<String>,
    edit_text: Signal<String>,
    editing: Signal<bool>,
    original_title: String,
    original_text: String,
    msg_id: MessageId,
    on_edit: Option<EventHandler<(MessageId, String, String)>>,
    size: MessageSize,
) -> Element {
    let (input_class, textarea_class, button_class, container_class) = match size {
        MessageSize::Compact => (
            "w-full p-2 rounded-lg text-sm bg-surface border border-border text-text",
            "w-full p-2 rounded-lg text-sm bg-surface border border-border text-text resize-y min-h-[80px]",
            "text-xs px-2 py-1 rounded",
            "space-y-2",
        ),
        _ => (
            "w-full p-3 rounded-lg bg-surface border border-border text-text",
            "w-full p-3 rounded-lg bg-surface border border-border text-text resize-y min-h-[120px]",
            "px-4 py-2 rounded-lg",
            "space-y-4",
        ),
    };

    let original_title_for_key = original_title.clone();
    let original_title_for_save = original_title.clone();
    let original_text_for_key = original_text.clone();
    let original_text_for_save = original_text.clone();
    let msg_id_for_key = msg_id.clone();
    let msg_id_for_save = msg_id.clone();

    // Use Ctrl+Enter for Card, Enter for Reply
    let use_ctrl = !matches!(size, MessageSize::Compact);

    // Check if content has changed
    let has_changes = move || {
        let new_title = edit_title.read().clone();
        let new_text = edit_text.read().clone();
        !new_text.is_empty() && (new_title != original_title_for_key || new_text != original_text_for_key)
    };

    rsx! {
        div { class: "{container_class}",
            // Title input
            input {
                r#type: "text",
                class: "{input_class}",
                placeholder: "Title (optional)",
                value: "{edit_title}",
                oninput: move |e| edit_title.set(e.value().clone()),
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Escape {
                        editing.set(false);
                    }
                },
            }
            // Content textarea
            textarea {
                class: "{textarea_class}",
                value: "{edit_text}",
                autofocus: true,
                oninput: move |e| edit_text.set(e.value().clone()),
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Escape {
                        editing.set(false);
                    } else if e.key() == Key::Enter {
                        let should_submit = if use_ctrl {
                            e.modifiers().ctrl() || e.modifiers().meta()
                        } else {
                            !e.modifiers().shift()
                        };
                        if should_submit {
                            e.prevent_default();
                            if has_changes() {
                                if let Some(ref handler) = on_edit {
                                    handler.call((msg_id_for_key.clone(), edit_title.read().clone(), edit_text.read().clone()));
                                }
                            }
                            editing.set(false);
                        }
                    }
                },
            }
            div { class: if matches!(size, MessageSize::Compact) { "flex gap-2" } else { "flex gap-3" },
                button {
                    class: "{button_class} bg-surface hover:bg-border text-text",
                    onclick: move |_| editing.set(false),
                    "Cancel"
                }
                button {
                    class: "{button_class} bg-accent text-white",
                    onclick: move |_| {
                        let new_title = edit_title.read().clone();
                        let new_text = edit_text.read().clone();
                        if !new_text.is_empty() && (new_title != original_title_for_save || new_text != original_text_for_save) {
                            if let Some(ref handler) = on_edit {
                                handler.call((msg_id_for_save.clone(), new_title, new_text));
                            }
                        }
                        editing.set(false);
                    },
                    "Save"
                }
            }
        }
    }
}

/// Shared reactions display component
#[component]
fn ReactionDisplay(
    reactions: HashMap<String, Vec<MemberId>>,
    self_member_id: MemberId,
    member_names: HashMap<MemberId, String>,
    msg_id: MessageId,
    on_react: Option<EventHandler<(MessageId, String)>>,
    size: MessageSize,
) -> Element {
    if reactions.is_empty() {
        return rsx! {};
    }

    let (container_class, badge_class, count_class) = match size {
        MessageSize::Compact => (
            "flex flex-wrap items-center gap-1 mt-2",
            "inline-flex items-center gap-0.5 text-sm",
            "text-xs text-text-muted",
        ),
        _ => (
            "flex flex-wrap items-center gap-2 px-6 pb-4",
            "inline-flex items-center gap-1 px-2 py-1 rounded-full bg-surface text-base",
            "text-sm text-text-muted",
        ),
    };

    let mut sorted: Vec<_> = reactions.iter().collect();
    sorted.sort_by_key(|(e, _)| e.as_str());

    rsx! {
        div { class: "{container_class}",
            {sorted.into_iter().map(|(emoji, reactors)| {
                let count = reactors.len();
                let is_user = reactors.contains(&self_member_id);
                let emoji_click = emoji.clone();
                let msg_id_click = msg_id.clone();
                let names: Vec<String> = reactors.iter().map(|id| {
                    if *id == self_member_id { "You".to_string() }
                    else { member_names.get(id).cloned().unwrap_or("Unknown".to_string()) }
                }).collect();
                let tooltip = names.join(", ");

                let highlight_class = if matches!(size, MessageSize::Compact) {
                    if is_user { "cursor-pointer underline decoration-accent" } else { "" }
                } else {
                    if is_user { "cursor-pointer ring-1 ring-accent" } else { "" }
                };

                rsx! {
                    span {
                        key: "{emoji}",
                        class: format!("{} {}", badge_class, highlight_class),
                        title: "{tooltip}",
                        onclick: move |e| {
                            e.stop_propagation();
                            if is_user {
                                if let Some(ref handler) = on_react {
                                    handler.call((msg_id_click.clone(), emoji_click.clone()));
                                }
                            }
                        },
                        "{emoji}"
                        if count > 1 {
                            span { class: "{count_class}", "{count}" }
                        }
                    }
                }
            })}
        }
    }
}

/// Shared action buttons component (hover menu)
#[component]
fn ActionButtons(
    msg_id: MessageId,
    is_self: bool,
    title_text: String,
    content_text: String,
    author_name: String,
    content_preview: String,
    editing: Signal<bool>,
    edit_title: Signal<String>,
    edit_text: Signal<String>,
    is_hovered: Signal<bool>,
    on_react: Option<EventHandler<(MessageId, String)>>,
    on_reply: Option<EventHandler<ReplyContext>>,
    on_edit: Option<EventHandler<(MessageId, String, String)>>,
    on_request_delete: Option<EventHandler<MessageId>>,
    size: MessageSize,
) -> Element {
    // Create emoji picker state locally to ensure it's element-specific
    let mut open_emoji_picker = use_signal(|| false);

    let has_actions = on_react.is_some() || on_reply.is_some() || (is_self && (on_edit.is_some() || on_request_delete.is_some()));

    if !has_actions || !*is_hovered.read() {
        return rsx! {};
    }

    let (container_class, button_class, picker_button_class, picker_grid_class) = match size {
        MessageSize::Compact => (
            "absolute right-2 top-2 flex gap-1 bg-panel rounded shadow border border-border px-1 py-0.5",
            "text-xs text-text-muted hover:text-accent px-1",
            "p-1 rounded hover:bg-surface text-lg",
            "grid gap-[2px]",
        ),
        _ => (
            "absolute right-4 top-4 flex gap-1 bg-panel rounded-lg shadow border border-border px-2 py-1",
            "text-sm text-text-muted hover:text-accent px-2",
            "p-2 rounded hover:bg-surface text-xl",
            "grid gap-1",
        ),
    };

    let msg_id_for_react = msg_id.clone();
    let msg_id_for_reply = msg_id.clone();
    let msg_id_for_delete = msg_id.clone();

    rsx! {
        div {
            class: "{container_class}",
            onclick: move |e| e.stop_propagation(),

            // React button with emoji picker
            if on_react.is_some() {
                div { class: "relative",
                    button {
                        class: "{button_class}",
                        onclick: move |_| open_emoji_picker.set(!open_emoji_picker()),
                        "+"
                    }
                    if *open_emoji_picker.read() {
                        div {
                            class: "fixed inset-0 z-40",
                            onclick: move |_| open_emoji_picker.set(false),
                        }
                        div {
                            class: "absolute right-0 top-full mt-1 p-1 bg-panel rounded shadow border border-border z-50 {picker_grid_class}",
                            style: "grid-template-columns: repeat(4, 1fr);",
                            {FREQUENT_EMOJIS.iter().map({
                                let msg_id = msg_id_for_react.clone();
                                move |emoji| {
                                    let e = emoji.to_string();
                                    let mid = msg_id.clone();
                                    rsx! {
                                        button {
                                            key: "{emoji}",
                                            class: "{picker_button_class}",
                                            onclick: move |_| {
                                                if let Some(ref handler) = on_react {
                                                    handler.call((mid.clone(), e.clone()));
                                                }
                                                open_emoji_picker.set(false);
                                            },
                                            "{emoji}"
                                        }
                                    }
                                }
                            })}
                        }
                    }
                }
            }

            // Reply button
            if let Some(ref reply_handler) = on_reply {
                button {
                    class: "{button_class}",
                    onclick: {
                        let handler = reply_handler.clone();
                        let msg_id = msg_id_for_reply.clone();
                        let author = author_name.clone();
                        let preview = content_preview.clone();
                        move |_| {
                            handler.call(ReplyContext {
                                message_id: msg_id.clone(),
                                author_name: author.clone(),
                                content_preview: preview.clone(),
                            });
                        }
                    },
                    "reply"
                }
            }

            // Edit/Delete buttons (only for own messages)
            if is_self {
                if on_edit.is_some() {
                    button {
                        class: if matches!(size, MessageSize::Compact) { "text-xs text-text-muted hover:text-text px-1" } else { "text-sm text-text-muted hover:text-text px-2" },
                        onclick: {
                            let title = title_text.clone();
                            let text = content_text.clone();
                            move |_| {
                                edit_title.set(title.clone());
                                edit_text.set(text.clone());
                                editing.set(true);
                            }
                        },
                        "edit"
                    }
                }
                if let Some(ref delete_handler) = on_request_delete {
                    button {
                        class: if matches!(size, MessageSize::Compact) { "text-xs text-text-muted hover:text-red-500 px-1" } else { "text-sm text-text-muted hover:text-red-500 px-2" },
                        onclick: {
                            let handler = delete_handler.clone();
                            let msg_id = msg_id_for_delete.clone();
                            move |_| handler.call(msg_id.clone())
                        },
                        "delete"
                    }
                }
            }
        }
    }
}

/// Unified component for rendering messages in either card or reply style
/// Replaces both ReplyTreeNode and PostCard
#[component]
pub fn MessageCard(
    /// The message to display
    message: MessageData,
    /// Display variant (Card or Reply)
    #[props(default)]
    variant: MessageCardVariant,
    /// Self member ID for determining edit/delete permissions
    self_member_id: MemberId,
    /// Member names lookup for reaction tooltips
    #[props(default)]
    member_names: HashMap<MemberId, String>,
    /// Nested replies (for Reply variant)
    #[props(default)]
    replies: Vec<MessageWithReplies>,
    /// Nesting depth for indentation (Reply variant only)
    #[props(default = 0)]
    depth: u32,
    /// Whether this is expanded view with larger styling (Card variant only)
    #[props(default = false)]
    expanded: bool,
    /// Click handler for card navigation (Card variant only)
    #[props(default)]
    on_click: Option<EventHandler<()>>,
    /// Handler for reactions
    #[props(default)]
    on_react: Option<EventHandler<(MessageId, String)>>,
    /// Handler for delete requests
    #[props(default)]
    on_request_delete: Option<EventHandler<MessageId>>,
    /// Handler for edits (message_id, new_title, new_content)
    #[props(default)]
    on_edit: Option<EventHandler<(MessageId, String, String)>>,
    /// Handler for replies
    #[props(default)]
    on_reply: Option<EventHandler<ReplyContext>>,
    /// Whether to show replies section below (Card variant only)
    #[props(default = false)]
    show_replies: bool,
    /// Default reply context for replies section
    #[props(default)]
    default_reply_to: Option<ReplyContext>,
) -> Element {
    let msg = message.clone();
    let is_self = msg.is_self;

    let timestamp_ms = msg.time.timestamp_millis();
    let time_str = format_utc_as_local_time(timestamp_ms);
    let full_time_str = if msg.time_clamped {
        format!(
            "{} (sender's clock may be incorrect)",
            format_utc_as_full_datetime(timestamp_ms)
        )
    } else {
        format_utc_as_full_datetime(timestamp_ms)
    };

    let editing = use_signal(|| false);
    let edit_title = use_signal(String::new);
    let edit_text = use_signal(String::new);
    let mut is_hovered = use_signal(|| false);

    let msg_id = msg.message_id.clone();
    let content_preview = msg.content_text.chars().take(100).collect::<String>();
    let author_id = msg.author_id;
    let author_name = msg.author_name.clone();
    let title_text = msg.title_text.clone();
    let content_html = msg.content_html.clone();
    let content_text = msg.content_text.clone();
    let edited = msg.edited;
    let reactions = msg.reactions.clone();

    // Determine size based on variant and expanded state
    let size = match variant {
        MessageCardVariant::Reply => MessageSize::Compact,
        MessageCardVariant::Card if expanded => MessageSize::Large,
        MessageCardVariant::Card => MessageSize::Normal,
    };

    match variant {
        MessageCardVariant::Reply => {
            // Indent based on depth (max 4 levels visually)
            let indent_class = match depth.min(2) {
                0 => "",
                1 => "ml-8",
                _ => "ml-16",
            };

            rsx! {
                div {
                    key: "{msg_id:?}",
                    class: "{indent_class}",
                    // Message card
                    div {
                        class: "relative border-l-2 pl-4 py-2 hover:bg-surface/30 transition-colors",
                        style: if is_self { "border-color: var(--accent);" } else { "border-color: var(--border);" },
                        onmouseenter: move |_| is_hovered.set(true),
                        onmouseleave: move |_| is_hovered.set(false),

                        // Header
                        div { class: "flex items-center gap-3 mb-2",
                            MessageHeader {
                                author_id: author_id,
                                author_name: author_name.clone(),
                                is_self: is_self,
                                time_str: time_str.clone(),
                                full_time_str: full_time_str.clone(),
                                time_clamped: msg.time_clamped,
                                size: size,
                            }
                        }

                        // Content (or edit form)
                        if *editing.read() {
                            MessageEditForm {
                                edit_title: edit_title,
                                edit_text: edit_text,
                                editing: editing,
                                original_title: title_text.clone(),
                                original_text: content_text.clone(),
                                msg_id: msg_id.clone(),
                                on_edit: on_edit.clone(),
                                size: size,
                            }
                        } else {
                            MessageContentDisplay {
                                title_text: title_text.clone(),
                                content_html: content_html.clone(),
                                edited: edited,
                                size: size,
                                expanded: false,
                            }
                        }

                        // Reactions
                        ReactionDisplay {
                            reactions: reactions.clone(),
                            self_member_id: self_member_id,
                            member_names: member_names.clone(),
                            msg_id: msg_id.clone(),
                            on_react: on_react.clone(),
                            size: size,
                        }

                        // Action buttons
                        ActionButtons {
                            msg_id: msg_id.clone(),
                            is_self: is_self,
                            title_text: title_text.clone(),
                            content_text: content_text.clone(),
                            author_name: author_name.clone(),
                            content_preview: content_preview.clone(),
                            editing: editing,
                            edit_title: edit_title,
                            edit_text: edit_text,
                            is_hovered: is_hovered,
                            on_react: on_react.clone(),
                            on_reply: on_reply.clone(),
                            on_edit: on_edit.clone(),
                            on_request_delete: on_request_delete.clone(),
                            size: size,
                        }
                    }

                    // Nested replies (recursive)
                    if !replies.is_empty() {
                        div { class: "mt-1",
                            {replies.iter().map({
                                let member_names = member_names.clone();
                                move |reply| {
                                    let member_names = member_names.clone();
                                    rsx! {
                                        MessageCard {
                                            message: reply.message.clone(),
                                            variant: MessageCardVariant::Reply,
                                            self_member_id: self_member_id,
                                            member_names: member_names,
                                            replies: reply.replies.clone(),
                                            depth: depth + 1,
                                            on_react: on_react.clone(),
                                            on_request_delete: on_request_delete.clone(),
                                            on_edit: on_edit.clone(),
                                            on_reply: on_reply.clone(),
                                        }
                                    }
                                }
                            })}
                        }
                    }
                }
            }
        }

        MessageCardVariant::Card => {
            let (header_padding, content_padding) = if expanded {
                ("px-8 py-6", "px-8 py-8")
            } else {
                ("px-6 py-4", "px-6 py-6")
            };

            let card_class = if on_click.is_some() {
                "bg-panel rounded-2xl border border-border shadow-sm overflow-hidden hover:border-accent/50 transition-colors cursor-pointer"
            } else {
                "bg-panel rounded-2xl border border-border shadow-sm overflow-hidden"
            };

            // Create reply context for when show_replies is enabled
            let msg_id_for_replies = msg_id.clone();
            let reply_context_for_section = if show_replies {
                Some(ReplyContext {
                    message_id: msg_id.clone(),
                    author_name: author_name.clone(),
                    content_preview: content_preview.clone(),
                })
            } else {
                None
            };

            rsx! {
                div {
                    key: "{msg_id:?}",
                    class: "relative",
                    onmouseenter: move |_| is_hovered.set(true),
                    onmouseleave: move |_| is_hovered.set(false),

                    div {
                        class: "{card_class}",
                        onclick: move |_| {
                            if let Some(ref handler) = on_click {
                                handler.call(());
                            }
                        },

                        // Header
                        div {
                            class: "flex items-center gap-4 {header_padding} border-b border-border bg-surface/30",
                            MessageHeader {
                                author_id: author_id,
                                author_name: author_name.clone(),
                                is_self: is_self,
                                time_str: time_str.clone(),
                                full_time_str: full_time_str.clone(),
                                time_clamped: msg.time_clamped,
                                size: size,
                            }
                        }

                        // Content
                        div { class: "{content_padding}",
                            if *editing.read() {
                                MessageEditForm {
                                    edit_title: edit_title,
                                    edit_text: edit_text,
                                    editing: editing,
                                    original_title: title_text.clone(),
                                    original_text: content_text.clone(),
                                    msg_id: msg_id.clone(),
                                    on_edit: on_edit.clone(),
                                    size: size,
                                }
                            } else {
                                MessageContentDisplay {
                                    title_text: title_text.clone(),
                                    content_html: content_html.clone(),
                                    edited: edited,
                                    size: size,
                                    expanded: expanded,
                                }
                            }
                        }

                        // Reactions
                        ReactionDisplay {
                            reactions: reactions.clone(),
                            self_member_id: self_member_id,
                            member_names: member_names.clone(),
                            msg_id: msg_id.clone(),
                            on_react: on_react.clone(),
                            size: size,
                        }

                        // Action buttons
                        ActionButtons {
                            msg_id: msg_id.clone(),
                            is_self: is_self,
                            title_text: title_text.clone(),
                            content_text: content_text.clone(),
                            author_name: author_name.clone(),
                            content_preview: content_preview.clone(),
                            editing: editing,
                            edit_title: edit_title,
                            edit_text: edit_text,
                            is_hovered: is_hovered,
                            on_react: on_react.clone(),
                            on_reply: on_reply.clone(),
                            on_edit: on_edit.clone(),
                            on_request_delete: on_request_delete.clone(),
                            size: size,
                        }
                    }

                    // Replies section (if enabled)
                    if show_replies {
                        div { class: "mt-4",
                            h3 { class: "text-lg font-semibold text-text-muted border-b border-border pb-2 mb-4",
                                "Replies"
                            }
                            Conversation {
                                parent_message_id: Some(msg_id_for_replies),
                                default_reply_to: reply_context_for_section.or(default_reply_to),
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Get all top-level posts (messages that are not replies)
pub fn get_top_level_posts(all_messages: &[MessageData]) -> Vec<MessageData> {
    let mut posts: Vec<_> = all_messages
        .iter()
        .filter(|m| m.is_top_level_post())
        .cloned()
        .collect();
    posts.sort_by_key(|m| std::cmp::Reverse(m.time));
    posts
}
