//! Shared message action handlers for reactions, edits, and deletions.

use crate::components::app::{NEEDS_SYNC, BOARDS};
use crate::util::ecies::encrypt_with_symmetric_key;
use crate::util::get_current_system_time;
use dioxus::prelude::*;
use ed25519_dalek::{SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use river_core::chat_delegate::BoardKey;
use river_core::board_state::content::ActionContentV1;
use river_core::board_state::member::MemberId;
use river_core::board_state::message::{AuthorizedMessageV1, MessageId, MessageV1, BoardMessageBody};
use river_core::board_state::{ChatBoardParametersV1, ChatBoardStateV1, ChatBoardStateV1Delta};

/// Parameters needed for message actions
#[derive(Clone)]
pub struct ActionContext {
    pub current_board: VerifyingKey,
    pub board_key: BoardKey,
    pub self_sk: SigningKey,
    pub board_state: ChatBoardStateV1,
    pub is_private: bool,
    pub secret_opt: Option<([u8; 32], u32)>,
}

impl ActionContext {
    /// Create an ActionContext from the current board data, if available
    pub fn from_current_board() -> Option<Self> {
        use crate::components::app::CURRENT_BOARD;

        let current_board = CURRENT_BOARD.read();
        if let Some(key) = current_board.owner_key {
            let boards = BOARDS.read();
            if let Some(board_data) = boards.map.get(&key) {
                return Some(Self {
                    current_board: key,
                    board_key: board_data.board_key(),
                    self_sk: board_data.self_sk.clone(),
                    board_state: board_data.board_state.clone(),
                    is_private: board_data.is_private(),
                    secret_opt: board_data.get_secret().map(|(s, v)| (*s, v)),
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
        let boards = BOARDS.read();
        boards.map.get(&ctx.current_board).and_then(|board_data| {
            board_data
                .board_state
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
            BoardMessageBody::private_action(ciphertext, nonce, version)
        } else {
            return;
        }
    } else {
        BoardMessageBody::delete(target_message_id)
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
            BoardMessageBody::private_action(ciphertext, nonce, version)
        } else {
            return;
        }
    } else {
        BoardMessageBody::edit(target_message_id, new_title, new_text)
    };

    send_action_messages(ctx, vec![content]).await;
}

// Helper to build remove reaction content
fn build_remove_reaction_content(
    ctx: &ActionContext,
    target_message_id: MessageId,
    emoji: String,
) -> Option<BoardMessageBody> {
    if ctx.is_private {
        let (secret, version) = ctx.secret_opt?;
        let action = ActionContentV1::remove_reaction(target_message_id, emoji);
        let action_bytes = action.encode();
        let (ciphertext, nonce) = encrypt_with_symmetric_key(&secret, &action_bytes);
        Some(BoardMessageBody::private_action(ciphertext, nonce, version))
    } else {
        Some(BoardMessageBody::remove_reaction(target_message_id, emoji))
    }
}

// Helper to build add reaction content
fn build_add_reaction_content(
    ctx: &ActionContext,
    target_message_id: MessageId,
    emoji: String,
) -> Option<BoardMessageBody> {
    if ctx.is_private {
        let (secret, version) = ctx.secret_opt?;
        let action = ActionContentV1::reaction(target_message_id, emoji);
        let action_bytes = action.encode();
        let (ciphertext, nonce) = encrypt_with_symmetric_key(&secret, &action_bytes);
        Some(BoardMessageBody::private_action(ciphertext, nonce, version))
    } else {
        Some(BoardMessageBody::reaction(target_message_id, emoji))
    }
}

// Send action messages and apply delta
async fn send_action_messages(ctx: ActionContext, contents: Vec<BoardMessageBody>) {
    if contents.is_empty() {
        return;
    }

    let mut auth_messages = Vec::new();
    for content in contents {
        let message = MessageV1 {
            board_owner: MemberId::from(ctx.current_board),
            author: MemberId::from(&ctx.self_sk.verifying_key()),
            content,
            time: get_current_system_time(),
        };

        let mut message_bytes = Vec::new();
        if ciborium::ser::into_writer(&message, &mut message_bytes).is_err() {
            return;
        }

        let signature = crate::signing::sign_message_with_fallback(
            ctx.board_key,
            message_bytes,
            &ctx.self_sk,
        )
        .await;

        auth_messages.push(AuthorizedMessageV1::with_signature(message, signature));
    }

    if !auth_messages.is_empty() {
        let delta = ChatBoardStateV1Delta {
            recent_messages: Some(auth_messages),
            ..Default::default()
        };

        let board_state_clone = ctx.board_state.clone();
        let current_board = ctx.current_board;

        BOARDS.with_mut(|boards| {
            if let Some(board_data) = boards.map.get_mut(&current_board) {
                if board_data
                    .board_state
                    .apply_delta(
                        &board_state_clone,
                        &ChatBoardParametersV1 { owner: current_board },
                        &Some(delta),
                    )
                    .is_ok()
                {
                    NEEDS_SYNC.write().insert(current_board);
                }
            }
        });
    }
}
