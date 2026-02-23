use crate::components::app::receive_times::get_delay_secs;
use crate::components::app::{CURRENT_ROOM, MEMBER_INFO_MODAL, NEEDS_SYNC, ROOMS};
use crate::util::avatar::get_avatar;
use crate::util::ecies::unseal_bytes_with_secrets;
use crate::util::markdown::text_to_html;
use crate::util::messaging::{send_message, ReplyContext};
use crate::util::{format_utc_as_full_datetime, format_utc_as_local_time, get_current_system_time};
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
use freenet_scaffold::ComposableState;
use river_core::room_state::member::MemberId;
use river_core::room_state::member_info::MemberInfoV1;
use river_core::room_state::message::{
    AuthorizedMessageV1, MessageId, MessageV1, MessagesV1, RoomMessageBody,
};
use river_core::room_state::{ChatRoomParametersV1, ChatRoomStateV1Delta};
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
    let handle_toggle_reaction = {
        let current_room_data = current_room_data.clone();
        move |target_message_id: MessageId, emoji: String| {
            if let (Some(current_room), Some(current_room_data)) =
                (CURRENT_ROOM.read().owner_key, current_room_data.clone())
            {
                let room_key = current_room_data.room_key();
                let self_sk = current_room_data.self_sk.clone();
                let room_state_clone = current_room_data.room_state.clone();
                let is_private = current_room_data.is_private();
                let secret_opt = current_room_data
                    .get_secret()
                    .map(|(secret, version)| (*secret, version));

                let self_member_id = MemberId::from(&self_sk.verifying_key());
                let existing_reaction: Option<String> = current_room_data
                    .room_state
                    .recent_messages
                    .reactions(&target_message_id)
                    .and_then(|reactions| {
                        reactions.iter().find_map(|(e, reactors)| {
                            if reactors.contains(&self_member_id) {
                                Some(e.clone())
                            } else {
                                None
                            }
                        })
                    });

                let clicked_same = existing_reaction.as_ref() == Some(&emoji);

                spawn_local(async move {
                    use crate::util::ecies::encrypt_with_symmetric_key;
                    use river_core::room_state::content::ActionContentV1;

                    let mut messages_to_send = Vec::new();

                    if clicked_same {
                        let content = if is_private {
                            if let Some((secret, version)) = &secret_opt {
                                let action = ActionContentV1::remove_reaction(
                                    target_message_id.clone(),
                                    emoji.clone(),
                                );
                                let action_bytes = action.encode();
                                let (ciphertext, nonce) =
                                    encrypt_with_symmetric_key(secret, &action_bytes);
                                RoomMessageBody::private_action(ciphertext, nonce, *version)
                            } else {
                                return;
                            }
                        } else {
                            RoomMessageBody::remove_reaction(target_message_id.clone(), emoji.clone())
                        };
                        messages_to_send.push(content);
                    } else {
                        if let Some(old_emoji) = existing_reaction {
                            let content = if is_private {
                                if let Some((secret, version)) = &secret_opt {
                                    let action = ActionContentV1::remove_reaction(
                                        target_message_id.clone(),
                                        old_emoji,
                                    );
                                    let action_bytes = action.encode();
                                    let (ciphertext, nonce) =
                                        encrypt_with_symmetric_key(secret, &action_bytes);
                                    RoomMessageBody::private_action(ciphertext, nonce, *version)
                                } else {
                                    return;
                                }
                            } else {
                                RoomMessageBody::remove_reaction(target_message_id.clone(), old_emoji)
                            };
                            messages_to_send.push(content);
                        }

                        let content = if is_private {
                            if let Some((secret, version)) = &secret_opt {
                                let action =
                                    ActionContentV1::reaction(target_message_id.clone(), emoji.clone());
                                let action_bytes = action.encode();
                                let (ciphertext, nonce) =
                                    encrypt_with_symmetric_key(secret, &action_bytes);
                                RoomMessageBody::private_action(ciphertext, nonce, *version)
                            } else {
                                return;
                            }
                        } else {
                            RoomMessageBody::reaction(target_message_id.clone(), emoji.clone())
                        };
                        messages_to_send.push(content);
                    }

                    let mut auth_messages = Vec::new();
                    for content in messages_to_send {
                        let message = MessageV1 {
                            room_owner: MemberId::from(current_room),
                            author: MemberId::from(&self_sk.verifying_key()),
                            content,
                            time: get_current_system_time(),
                        };

                        let mut message_bytes = Vec::new();
                        if ciborium::ser::into_writer(&message, &mut message_bytes).is_err() {
                            return;
                        }

                        let signature = crate::signing::sign_message_with_fallback(
                            room_key,
                            message_bytes,
                            &self_sk,
                        )
                        .await;

                        auth_messages.push(AuthorizedMessageV1::with_signature(message, signature));
                    }

                    if !auth_messages.is_empty() {
                        let delta = ChatRoomStateV1Delta {
                            recent_messages: Some(auth_messages),
                            ..Default::default()
                        };
                        ROOMS.with_mut(|rooms| {
                            if let Some(room_data) = rooms.map.get_mut(&current_room) {
                                if room_data
                                    .room_state
                                    .apply_delta(
                                        &room_state_clone,
                                        &ChatRoomParametersV1 { owner: current_room },
                                        &Some(delta),
                                    )
                                    .is_ok()
                                {
                                    NEEDS_SYNC.write().insert(current_room);
                                }
                            }
                        });
                    }
                });
            }
        }
    };

    // Handler for deleting a message
    let handle_delete_message = {
        let current_room_data = current_room_data.clone();
        move |target_message_id: MessageId| {
            if let (Some(current_room), Some(current_room_data)) =
                (CURRENT_ROOM.read().owner_key, current_room_data.clone())
            {
                let room_key = current_room_data.room_key();
                let self_sk = current_room_data.self_sk.clone();
                let room_state_clone = current_room_data.room_state.clone();
                let is_private = current_room_data.is_private();
                let secret_opt = current_room_data
                    .get_secret()
                    .map(|(secret, version)| (*secret, version));

                spawn_local(async move {
                    use crate::util::ecies::encrypt_with_symmetric_key;
                    use river_core::room_state::content::ActionContentV1;

                    let content = if is_private {
                        if let Some((secret, version)) = secret_opt {
                            let action = ActionContentV1::delete(target_message_id.clone());
                            let action_bytes = action.encode();
                            let (ciphertext, nonce) =
                                encrypt_with_symmetric_key(&secret, &action_bytes);
                            RoomMessageBody::private_action(ciphertext, nonce, version)
                        } else {
                            return;
                        }
                    } else {
                        RoomMessageBody::delete(target_message_id)
                    };

                    let message = MessageV1 {
                        room_owner: MemberId::from(current_room),
                        author: MemberId::from(&self_sk.verifying_key()),
                        content,
                        time: get_current_system_time(),
                    };

                    let mut message_bytes = Vec::new();
                    if ciborium::ser::into_writer(&message, &mut message_bytes).is_err() {
                        return;
                    }

                    let signature = crate::signing::sign_message_with_fallback(
                        room_key,
                        message_bytes,
                        &self_sk,
                    )
                    .await;

                    let auth_message = AuthorizedMessageV1::with_signature(message, signature);
                    let delta = ChatRoomStateV1Delta {
                        recent_messages: Some(vec![auth_message]),
                        ..Default::default()
                    };
                    ROOMS.with_mut(|rooms| {
                        if let Some(room_data) = rooms.map.get_mut(&current_room) {
                            if room_data
                                .room_state
                                .apply_delta(
                                    &room_state_clone,
                                    &ChatRoomParametersV1 { owner: current_room },
                                    &Some(delta),
                                )
                                .is_ok()
                            {
                                NEEDS_SYNC.write().insert(current_room);
                            }
                        }
                    });
                });
            }
        }
    };

    // Handler for editing a message
    let handle_edit_message = {
        let current_room_data = current_room_data.clone();
        move |target_message_id: MessageId, new_text: String| {
            if new_text.is_empty() {
                return;
            }
            if let (Some(current_room), Some(current_room_data)) =
                (CURRENT_ROOM.read().owner_key, current_room_data.clone())
            {
                let room_key = current_room_data.room_key();
                let self_sk = current_room_data.self_sk.clone();
                let room_state_clone = current_room_data.room_state.clone();
                let is_private = current_room_data.is_private();
                let secret_opt = current_room_data
                    .get_secret()
                    .map(|(secret, version)| (*secret, version));

                spawn_local(async move {
                    use crate::util::ecies::encrypt_with_symmetric_key;
                    use river_core::room_state::content::ActionContentV1;

                    let content = if is_private {
                        if let Some((secret, version)) = secret_opt {
                            let action = ActionContentV1::edit(target_message_id.clone(), new_text);
                            let action_bytes = action.encode();
                            let (ciphertext, nonce) =
                                encrypt_with_symmetric_key(&secret, &action_bytes);
                            RoomMessageBody::private_action(ciphertext, nonce, version)
                        } else {
                            return;
                        }
                    } else {
                        RoomMessageBody::edit(target_message_id, new_text)
                    };

                    let message = MessageV1 {
                        room_owner: MemberId::from(current_room),
                        author: MemberId::from(&self_sk.verifying_key()),
                        content,
                        time: get_current_system_time(),
                    };

                    let mut message_bytes = Vec::new();
                    if ciborium::ser::into_writer(&message, &mut message_bytes).is_err() {
                        return;
                    }

                    let signature = crate::signing::sign_message_with_fallback(
                        room_key,
                        message_bytes,
                        &self_sk,
                    )
                    .await;

                    let auth_message = AuthorizedMessageV1::with_signature(message, signature);
                    let delta = ChatRoomStateV1Delta {
                        recent_messages: Some(vec![auth_message]),
                        ..Default::default()
                    };
                    ROOMS.with_mut(|rooms| {
                        if let Some(room_data) = rooms.map.get_mut(&current_room) {
                            if room_data
                                .room_state
                                .apply_delta(
                                    &room_state_clone,
                                    &ChatRoomParametersV1 { owner: current_room },
                                    &Some(delta),
                                )
                                .is_ok()
                            {
                                NEEDS_SYNC.write().insert(current_room);
                            }
                        }
                    });
                });
            }
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
                                                        ReplyTreeNode {
                                                            message_with_replies: msg_with_replies,
                                                            self_member_id: self_member_id,
                                                            member_names: member_names,
                                                            depth: 0,
                                                            on_react: move |(msg_id, emoji)| {
                                                                handle_toggle_reaction(msg_id, emoji);
                                                            },
                                                            on_request_delete: move |msg_id| {
                                                                pending_delete.set(Some(msg_id));
                                                            },
                                                            on_edit: move |(msg_id, new_text)| {
                                                                handle_edit_message(msg_id, new_text);
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
                                PostInput {
                                    handle_send_message: move |msg: (String, String, Option<ReplyContext>)| {
                                        let mut handle = handle_send_message.clone();
                                        handle(msg)
                                    },
                                    replying_to: replying_to,
                                    on_request_edit_last: move |_| {},
                                    default_reply_to: default_reply_to.clone(),
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
                                    "You have been banned from sending messages in this room."
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

/// Recursive component for rendering a message with its nested replies
#[component]
fn ReplyTreeNode(
    message_with_replies: MessageWithReplies,
    self_member_id: MemberId,
    member_names: HashMap<MemberId, String>,
    depth: u32,
    on_react: EventHandler<(MessageId, String)>,
    on_request_delete: EventHandler<MessageId>,
    on_edit: EventHandler<(MessageId, String)>,
    on_reply: EventHandler<ReplyContext>,
) -> Element {
    // Clone everything we need to avoid borrow issues
    let msg = message_with_replies.message.clone();
    let replies = message_with_replies.replies.clone();
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

    let mut editing = use_signal(|| false);
    let mut edit_text = use_signal(String::new);
    let mut open_emoji_picker = use_signal(|| false);

    // Indent based on depth (max 4 levels visually)
    let indent_class = match depth.min(4) {
        0 => "",
        1 => "ml-8",
        2 => "ml-16",
        3 => "ml-24",
        _ => "ml-32",
    };

    let msg_id_for_key = msg.message_id.clone();
    let msg_id_for_edit_1 = msg.message_id.clone();
    let msg_id_for_edit_2 = msg.message_id.clone();
    let msg_id_for_delete = msg.message_id.clone();
    let msg_id_for_reply = msg.message_id.clone();
    let msg_id_for_react = msg.message_id.clone();
    let msg_id_for_react_picker = msg.message_id.clone();
    let author_name_for_reply = msg.author_name.clone();
    let content_preview = msg.content_text.chars().take(100).collect::<String>();
    let author_id = msg.author_id;
    let author_name = msg.author_name.clone();
    let title_text = msg.title_text.clone();
    let content_html = msg.content_html.clone();
    let content_text_for_edit = msg.content_text.clone();
    let content_text_for_edit_2 = msg.content_text.clone();
    let edited = msg.edited;
    let reactions = msg.reactions.clone();

    rsx! {
        div {
            key: "{msg_id_for_key:?}",
            class: "{indent_class}",
            // Message card
            div {
                class: "group relative border-l-2 pl-4 py-2 hover:bg-surface/30 transition-colors",
                style: if is_self { "border-color: var(--accent);" } else { "border-color: var(--border);" },

                // Header: avatar, name, time
                div { class: "flex items-center gap-3 mb-2",
                    img {
                        src: "{get_avatar(&author_id)}",
                        alt: "Avatar",
                        class: "w-8 h-8 rounded-full"
                    }
                    span {
                        class: "text-sm font-medium text-text cursor-pointer hover:text-accent transition-colors",
                        onclick: move |_| {
                            MEMBER_INFO_MODAL.with_mut(|signal| {
                                signal.member = Some(author_id);
                            });
                        },
                        "{author_name}"
                    }
                    if is_self {
                        span { class: "text-xs text-accent", "(you)" }
                    }
                    span {
                        class: if msg.time_clamped { "text-xs text-text-muted italic" } else { "text-xs text-text-muted" },
                        title: "{full_time_str}",
                        "{time_str}"
                    }
                }

                // Content (or edit form)
                if *editing.read() {
                    div { class: "space-y-2",
                        textarea {
                            class: "w-full p-2 rounded-lg text-sm bg-surface border border-border text-text resize-y min-h-[80px]",
                            value: "{edit_text}",
                            autofocus: true,
                            oninput: move |e| edit_text.set(e.value().clone()),
                            onkeydown: {
                                let original = content_text_for_edit.clone();
                                let msg_id = msg_id_for_edit_1.clone();
                                move |e: KeyboardEvent| {
                                    if e.key() == Key::Escape {
                                        editing.set(false);
                                    } else if e.key() == Key::Enter && !e.modifiers().shift() {
                                        e.prevent_default();
                                        let new_text = edit_text.read().clone();
                                        if !new_text.is_empty() && new_text != original {
                                            on_edit.call((msg_id.clone(), new_text));
                                        }
                                        editing.set(false);
                                    }
                                }
                            },
                        }
                        div { class: "flex gap-2",
                            button {
                                class: "text-xs px-2 py-1 rounded bg-surface hover:bg-border text-text",
                                onclick: move |_| editing.set(false),
                                "Cancel"
                            }
                            button {
                                class: "text-xs px-2 py-1 rounded bg-accent text-white",
                                onclick: {
                                    let original = content_text_for_edit_2.clone();
                                    let msg_id = msg_id_for_edit_2.clone();
                                    move |_| {
                                        let new_text = edit_text.read().clone();
                                        if !new_text.is_empty() && new_text != original {
                                            on_edit.call((msg_id.clone(), new_text));
                                        }
                                        editing.set(false);
                                    }
                                },
                                "Save"
                            }
                        }
                    }
                } else {
                    div {
                        // Title
                        if !title_text.is_empty() {
                            h4 { class: "font-semibold text-text mb-1",
                                "{title_text}"
                            }
                        }
                        // Content
                        div { class: "text-sm text-text",
                            span {
                                class: "prose prose-sm dark:prose-invert max-w-none",
                                dangerous_inner_html: "{content_html}"
                            }
                            if edited {
                                span { class: "text-xs ml-2 text-text-muted", "(edited)" }
                            }
                        }
                    }
                }

                // Reactions
                if !reactions.is_empty() {
                    div { class: "flex flex-wrap items-center gap-1 mt-2",
                        {
                            let mut sorted: Vec<_> = reactions.iter().collect();
                            sorted.sort_by_key(|(e, _)| e.as_str());
                            sorted.into_iter().map(|(emoji, reactors)| {
                                let count = reactors.len();
                                let is_user = reactors.contains(&self_member_id);
                                let emoji_click = emoji.clone();
                                let msg_id_click = msg_id_for_react.clone();
                                let names: Vec<String> = reactors.iter().map(|id| {
                                    if *id == self_member_id { "You".to_string() }
                                    else { member_names.get(id).cloned().unwrap_or("Unknown".to_string()) }
                                }).collect();
                                let tooltip = names.join(", ");
                                rsx! {
                                    span {
                                        key: "{emoji}",
                                        class: format!(
                                            "inline-flex items-center gap-0.5 text-sm {}",
                                            if is_user { "cursor-pointer underline decoration-accent" } else { "" }
                                        ),
                                        title: "{tooltip}",
                                        onclick: move |_| {
                                            if is_user {
                                                on_react.call((msg_id_click.clone(), emoji_click.clone()));
                                            }
                                        },
                                        "{emoji}"
                                        if count > 1 {
                                            span { class: "text-xs text-text-muted", "{count}" }
                                        }
                                    }
                                }
                            })
                        }
                    }
                }

                // Action buttons (hover)
                div {
                    class: "absolute right-2 top-2 opacity-0 group-hover:opacity-100 transition-opacity flex gap-1 bg-panel rounded shadow border border-border px-1 py-0.5",
                    // React button
                    div { class: "relative",
                        button {
                            class: "text-xs text-text-muted hover:text-accent px-1",
                            onclick: move |_| open_emoji_picker.set(!open_emoji_picker()),
                            "+"
                        }
                        if *open_emoji_picker.read() {
                            div {
                                class: "fixed inset-0 z-40",
                                onclick: move |_| open_emoji_picker.set(false),
                            }
                            div {
                                class: "absolute right-0 top-full mt-1 p-1 bg-panel rounded shadow border border-border z-50 grid",
                                style: "grid-template-columns: repeat(4, 1fr); gap: 2px;",
                                {FREQUENT_EMOJIS.iter().map({
                                    let msg_id = msg_id_for_react_picker.clone();
                                    move |emoji| {
                                        let e = emoji.to_string();
                                        let mid = msg_id.clone();
                                        rsx! {
                                            button {
                                                key: "{emoji}",
                                                class: "p-1 rounded hover:bg-surface text-lg",
                                                onclick: move |_| {
                                                    on_react.call((mid.clone(), e.clone()));
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
                    button {
                        class: "text-xs text-text-muted hover:text-accent px-1",
                        onclick: move |_| {
                            on_reply.call(ReplyContext {
                                message_id: msg_id_for_reply.clone(),
                                author_name: author_name_for_reply.clone(),
                                content_preview: content_preview.clone(),
                            });
                        },
                        "reply"
                    }
                    if is_self {
                        button {
                            class: "text-xs text-text-muted hover:text-text px-1",
                            onclick: {
                                let text = msg.content_text.clone();
                                move |_| {
                                    edit_text.set(text.clone());
                                    editing.set(true);
                                }
                            },
                            "edit"
                        }
                        button {
                            class: "text-xs text-text-muted hover:text-red-500 px-1",
                            onclick: move |_| on_request_delete.call(msg_id_for_delete.clone()),
                            "delete"
                        }
                    }
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
                                ReplyTreeNode {
                                    message_with_replies: reply.clone(),
                                    self_member_id: self_member_id,
                                    member_names: member_names,
                                    depth: depth + 1,
                                    on_react: move |(id, e)| on_react.call((id, e)),
                                    on_request_delete: move |id| on_request_delete.call(id),
                                    on_edit: move |(id, t)| on_edit.call((id, t)),
                                    on_reply: move |ctx| on_reply.call(ctx),
                                }
                            }
                        }
                    })}
                }
            }
        }
    }
}

/// Shared component for displaying a single post card
/// Used by both PostsView (list) and SinglePostView (detail)
#[component]
pub fn PostCard(
    /// The message data to display
    message: MessageData,
    /// Whether this is the expanded/detail view (larger styling)
    #[props(default = false)]
    expanded: bool,
    /// Whether to show replies under this post
    #[props(default = false)]
    show_replies: bool,
    /// Optional click handler for the card (used in list view for navigation)
    #[props(default)]
    on_click: Option<EventHandler<()>>,
) -> Element {
    let author_id = message.author_id;
    let author_name = message.author_name.clone();
    let title = message.title_text.clone();
    let content_text = message.content_text.clone();
    let content_html = message.content_html.clone();
    let time_clamped = message.time_clamped;
    let message_id = message.message_id.clone();

    // Create reply context for when show_replies is enabled
    let reply_context = if show_replies {
        Some(ReplyContext {
            message_id: message_id.clone(),
            author_name: author_name.clone(),
            content_preview: content_text.chars().take(100).collect(),
        })
    } else {
        None
    };

    let timestamp_ms = message.time.timestamp_millis();
    let time_str = format_utc_as_local_time(timestamp_ms);
    let full_time_str = if time_clamped {
        format!(
            "{} (sender's clock may be incorrect)",
            format_utc_as_full_datetime(timestamp_ms)
        )
    } else {
        format_utc_as_full_datetime(timestamp_ms)
    };

    // Styling based on expanded or list view
    let (avatar_size, header_padding, content_padding, title_class, content_class) = if expanded {
        (
            "w-16 h-16",
            "px-8 py-6",
            "px-8 py-8",
            "text-3xl font-bold text-text mb-6",
            "text-xl text-text leading-relaxed prose prose-xl dark:prose-invert max-w-none",
        )
    } else {
        (
            "w-14 h-14",
            "px-6 py-4",
            "px-6 py-6",
            "text-2xl font-bold text-text mb-4",
            "text-lg text-text leading-relaxed prose prose-lg dark:prose-invert max-w-none",
        )
    };

    let card_class = if on_click.is_some() {
        "bg-panel rounded-2xl border border-border shadow-sm overflow-hidden hover:border-accent/50 transition-colors cursor-pointer"
    } else {
        "bg-panel rounded-2xl border border-border shadow-sm overflow-hidden"
    };

    rsx! {
        div {
            class: "{card_class}",
            onclick: move |_| {
                if let Some(handler) = &on_click {
                    handler.call(());
                }
            },

            // Post header with author
            div {
                class: "flex items-center gap-4 {header_padding} border-b border-border bg-surface/30",
                img {
                    src: "{get_avatar(&author_id)}",
                    alt: "Avatar",
                    class: "{avatar_size} rounded-full"
                }
                div { class: "flex-1 min-w-0",
                    div {
                        class: if expanded {
                            "text-xl font-semibold text-text cursor-pointer hover:text-accent transition-colors"
                        } else {
                            "text-lg font-semibold text-text"
                        },
                        onclick: move |e| {
                            e.stop_propagation();
                            MEMBER_INFO_MODAL.with_mut(|signal| {
                                signal.member = Some(author_id);
                            });
                        },
                        "{author_name}"
                    }
                    span {
                        class: if time_clamped {
                            "text-sm text-text-muted italic"
                        } else {
                            "text-sm text-text-muted"
                        },
                        title: "{full_time_str}",
                        if time_clamped { "~{time_str}" } else { "{time_str}" }
                    }
                }
            }

            // Post content
            div { class: "{content_padding}",
                // Title
                if !title.is_empty() {
                    if expanded {
                        h1 { class: "{title_class}", "{title}" }
                    } else {
                        h2 { class: "{title_class}", "{title}" }
                    }
                }
                // Content
                div {
                    class: "{content_class}",
                    span { dangerous_inner_html: "{content_html}" }
                }
            }
        }

        // Replies section (if enabled)
        if show_replies {
            div { class: "mt-4",
                h3 { class: "text-lg font-semibold text-text-muted border-b border-border pb-2 mb-4",
                    "Replies"
                }
                Conversation {
                    parent_message_id: Some(message_id),
                    default_reply_to: reply_context,
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
