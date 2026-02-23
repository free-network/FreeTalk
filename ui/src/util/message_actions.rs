//! Shared message action handlers for reactions, edits, and deletions.

use crate::components::app::{NEEDS_SYNC, ROOMS};
use crate::util::ecies::encrypt_with_symmetric_key;
use crate::util::get_current_system_time;
use dioxus::prelude::*;
use ed25519_dalek::{SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use river_core::chat_delegate::RoomKey;
use river_core::room_state::content::ActionContentV1;
use river_core::room_state::member::MemberId;
use river_core::room_state::message::{AuthorizedMessageV1, MessageId, MessageV1, RoomMessageBody};
use river_core::room_state::{ChatRoomParametersV1, ChatRoomStateV1, ChatRoomStateV1Delta};

/// Parameters needed for message actions
#[derive(Clone)]
pub struct ActionContext {
    pub current_room: VerifyingKey,
    pub room_key: RoomKey,
    pub self_sk: SigningKey,
    pub room_state: ChatRoomStateV1,
    pub is_private: bool,
    pub secret_opt: Option<([u8; 32], u32)>,
}

impl ActionContext {
    /// Create an ActionContext from the current room data, if available
    pub fn from_current_room() -> Option<Self> {
        use crate::components::app::CURRENT_ROOM;

        let current_room = CURRENT_ROOM.read();
        if let Some(key) = current_room.owner_key {
            let rooms = ROOMS.read();
            if let Some(room_data) = rooms.map.get(&key) {
                return Some(Self {
                    current_room: key,
                    room_key: room_data.room_key(),
                    self_sk: room_data.self_sk.clone(),
                    room_state: room_data.room_state.clone(),
                    is_private: room_data.is_private(),
                    secret_opt: room_data.get_secret().map(|(s, v)| (*s, v)),
                });
            }
        }
        None
    }
}

/// Toggle a reaction on a message (add or remove)
pub async fn toggle_reaction(ctx: ActionContext, target_message_id: MessageId, emoji: String) {
    let self_member_id = MemberId::from(&ctx.self_sk.verifying_key());

    // Check if user already has this reaction
    let existing_reaction: Option<String> = {
        let rooms = ROOMS.read();
        rooms.map.get(&ctx.current_room).and_then(|room_data| {
            room_data
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
                })
        })
    };

    let clicked_same = existing_reaction.as_ref() == Some(&emoji);
    let mut messages_to_send = Vec::new();

    if clicked_same {
        // Remove the reaction
        let content = build_remove_reaction_content(
            &ctx,
            target_message_id.clone(),
            emoji,
        );
        if let Some(content) = content {
            messages_to_send.push(content);
        }
    } else {
        // Remove old reaction if exists, then add new one
        if let Some(old_emoji) = existing_reaction {
            let content = build_remove_reaction_content(
                &ctx,
                target_message_id.clone(),
                old_emoji,
            );
            if let Some(content) = content {
                messages_to_send.push(content);
            }
        }

        let content = build_add_reaction_content(&ctx, target_message_id.clone(), emoji);
        if let Some(content) = content {
            messages_to_send.push(content);
        }
    }

    send_action_messages(ctx, messages_to_send).await;
}

/// Delete a message
pub async fn delete_message(ctx: ActionContext, target_message_id: MessageId) {
    let content = if ctx.is_private {
        if let Some((secret, version)) = ctx.secret_opt {
            let action = ActionContentV1::delete(target_message_id);
            let action_bytes = action.encode();
            let (ciphertext, nonce) = encrypt_with_symmetric_key(&secret, &action_bytes);
            RoomMessageBody::private_action(ciphertext, nonce, version)
        } else {
            return;
        }
    } else {
        RoomMessageBody::delete(target_message_id)
    };

    send_action_messages(ctx, vec![content]).await;
}

/// Edit a message
pub async fn edit_message(ctx: ActionContext, target_message_id: MessageId, new_title: String, new_text: String) {
    if new_text.is_empty() {
        return;
    }

    let content = if ctx.is_private {
        if let Some((secret, version)) = ctx.secret_opt {
            let action = ActionContentV1::edit(target_message_id, new_title, new_text);
            let action_bytes = action.encode();
            let (ciphertext, nonce) = encrypt_with_symmetric_key(&secret, &action_bytes);
            RoomMessageBody::private_action(ciphertext, nonce, version)
        } else {
            return;
        }
    } else {
        RoomMessageBody::edit(target_message_id, new_title, new_text)
    };

    send_action_messages(ctx, vec![content]).await;
}

// Helper to build remove reaction content
fn build_remove_reaction_content(
    ctx: &ActionContext,
    target_message_id: MessageId,
    emoji: String,
) -> Option<RoomMessageBody> {
    if ctx.is_private {
        let (secret, version) = ctx.secret_opt?;
        let action = ActionContentV1::remove_reaction(target_message_id, emoji);
        let action_bytes = action.encode();
        let (ciphertext, nonce) = encrypt_with_symmetric_key(&secret, &action_bytes);
        Some(RoomMessageBody::private_action(ciphertext, nonce, version))
    } else {
        Some(RoomMessageBody::remove_reaction(target_message_id, emoji))
    }
}

// Helper to build add reaction content
fn build_add_reaction_content(
    ctx: &ActionContext,
    target_message_id: MessageId,
    emoji: String,
) -> Option<RoomMessageBody> {
    if ctx.is_private {
        let (secret, version) = ctx.secret_opt?;
        let action = ActionContentV1::reaction(target_message_id, emoji);
        let action_bytes = action.encode();
        let (ciphertext, nonce) = encrypt_with_symmetric_key(&secret, &action_bytes);
        Some(RoomMessageBody::private_action(ciphertext, nonce, version))
    } else {
        Some(RoomMessageBody::reaction(target_message_id, emoji))
    }
}

// Send action messages and apply delta
async fn send_action_messages(ctx: ActionContext, contents: Vec<RoomMessageBody>) {
    if contents.is_empty() {
        return;
    }

    let mut auth_messages = Vec::new();
    for content in contents {
        let message = MessageV1 {
            room_owner: MemberId::from(ctx.current_room),
            author: MemberId::from(&ctx.self_sk.verifying_key()),
            content,
            time: get_current_system_time(),
        };

        let mut message_bytes = Vec::new();
        if ciborium::ser::into_writer(&message, &mut message_bytes).is_err() {
            return;
        }

        let signature = crate::signing::sign_message_with_fallback(
            ctx.room_key,
            message_bytes,
            &ctx.self_sk,
        )
        .await;

        auth_messages.push(AuthorizedMessageV1::with_signature(message, signature));
    }

    if !auth_messages.is_empty() {
        let delta = ChatRoomStateV1Delta {
            recent_messages: Some(auth_messages),
            ..Default::default()
        };

        let room_state_clone = ctx.room_state.clone();
        let current_room = ctx.current_room;

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
}
