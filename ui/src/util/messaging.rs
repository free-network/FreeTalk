//! Message sending utilities for the UI.

use crate::components::app::notifications::request_permission_on_first_message;
use crate::components::app::{mark_needs_sync, BOARDS};
use crate::util::ecies::encrypt_with_symmetric_key;
use crate::util::get_current_system_time;
use dioxus::logger::tracing::{error, info, warn};
use dioxus::prelude::ReadableExt;
use ed25519_dalek::{SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use river_core::board_state::content::{
    ReplyContentV1, TextContentV1, CONTENT_TYPE_REPLY, CONTENT_TYPE_TEXT, REPLY_CONTENT_VERSION,
    TEXT_CONTENT_VERSION,
};
use river_core::board_state::member::MemberId;
use river_core::board_state::message::{
    AuthorizedMessageV1, BoardMessageBody, MessageId, MessageV1,
};
use river_core::board_state::{ChatBoardParametersV1, ChatBoardStateV1, ChatBoardStateV1Delta};
use river_core::chat_delegate::BoardKey;

/// Context for a reply message
#[derive(Clone, PartialEq, Debug)]
pub struct ReplyContext {
    pub message_id: MessageId,
    pub author_name: String,
    pub content_preview: String,
}

/// Send a message to a board.
///
/// This handles both regular text messages and replies, as well as
/// encryption for private boards.
pub async fn send_message(
    current_board: VerifyingKey,
    board_key: BoardKey,
    self_sk: SigningKey,
    board_state_clone: ChatBoardStateV1,
    is_private: bool,
    secret_opt: Option<([u8; 32], u32)>,
    title_text: String,
    message_text: String,
    reply_ctx: Option<ReplyContext>,
) {
    if message_text.is_empty() {
        warn!("Message is empty");
        return;
    }

    // Build content based on whether this is a reply or regular message
    let content = if let Some(reply) = reply_ctx {
        // Reply message
        if is_private {
            if let Some((secret, version)) = secret_opt {
                let reply_content = ReplyContentV1::new(
                    title_text.clone(),
                    message_text.clone(),
                    reply.message_id,
                    reply.author_name,
                    reply.content_preview,
                );
                let content_bytes = reply_content.encode();
                let (ciphertext, nonce) = encrypt_with_symmetric_key(&secret, &content_bytes);
                BoardMessageBody::private(
                    CONTENT_TYPE_REPLY,
                    REPLY_CONTENT_VERSION,
                    ciphertext,
                    nonce,
                    version,
                )
            } else {
                warn!("Board is private but no secret available, sending reply as public");
                BoardMessageBody::reply(
                    title_text.clone(),
                    message_text.clone(),
                    reply.message_id,
                    reply.author_name,
                    reply.content_preview,
                )
            }
        } else {
            BoardMessageBody::reply(
                title_text.clone(),
                message_text.clone(),
                reply.message_id,
                reply.author_name,
                reply.content_preview,
            )
        }
    } else {
        // Regular text message
        if is_private {
            if let Some((secret, version)) = secret_opt {
                let text_content = TextContentV1::new(title_text.clone(), message_text.clone());
                let content_bytes = text_content.encode();
                let (ciphertext, nonce) = encrypt_with_symmetric_key(&secret, &content_bytes);
                BoardMessageBody::private(
                    CONTENT_TYPE_TEXT,
                    TEXT_CONTENT_VERSION,
                    ciphertext,
                    nonce,
                    version,
                )
            } else {
                warn!("Board is private but no secret available, sending as public");
                BoardMessageBody::public(title_text.clone(), message_text.clone())
            }
        } else {
            BoardMessageBody::public(title_text.clone(), message_text.clone())
        }
    };

    let message = MessageV1 {
        board_owner: MemberId::from(current_board),
        author: MemberId::from(&self_sk.verifying_key()),
        content,
        time: get_current_system_time(),
    };

    // Serialize message to CBOR for signing
    let mut message_bytes = Vec::new();
    if let Err(e) = ciborium::ser::into_writer(&message, &mut message_bytes) {
        error!("Failed to serialize message for signing: {:?}", e);
        return;
    }

    // Sign using delegate with fallback to local signing
    let signature =
        crate::signing::sign_message_with_fallback(board_key, message_bytes, &self_sk).await;

    let auth_message = AuthorizedMessageV1::with_signature(message, signature);

    // Check if we need to re-add ourselves (pruned for inactivity)
    let (members_delta, member_info_delta) = {
        let boards_read = BOARDS.read();
        if let Some(board_data) = boards_read.map.get(&current_board) {
            let self_vk = board_data.self_sk.verifying_key();
            let is_in_members = self_vk == current_board
                || board_data
                    .board_state
                    .members
                    .members
                    .iter()
                    .any(|m| m.member.member_vk == self_vk);

            if !is_in_members {
                if let Some(ref authorized_member) = board_data.self_authorized_member {
                    let current_member_ids: std::collections::HashSet<_> = board_data
                        .board_state
                        .members
                        .members
                        .iter()
                        .map(|m| m.member.id())
                        .collect();
                    let mut members_to_add: Vec<river_core::board_state::member::AuthorizedMember> =
                        vec![authorized_member.clone()];
                    for chain_member in &board_data.invite_chain {
                        if !current_member_ids.contains(&chain_member.member.id()) {
                            members_to_add.push(chain_member.clone());
                        }
                    }

                    // Use stored member_info to preserve nickname, or fall back to "Member"
                    use river_core::board_state::member_info::{AuthorizedMemberInfo, MemberInfo};
                    let authorized_info: AuthorizedMemberInfo = if let Some(ref stored_info) =
                        board_data.self_member_info
                    {
                        stored_info.clone()
                    } else {
                        use river_core::board_state::privacy::SealedBytes;
                        let member_id = MemberId::from(&self_vk);
                        let existing_version = board_data
                            .board_state
                            .member_info
                            .member_info
                            .iter()
                            .find(|i| i.member_info.member_id == member_id)
                            .map(|i| i.member_info.version)
                            .unwrap_or(0);
                        let member_info = MemberInfo {
                            member_id,
                            version: existing_version,
                            preferred_nickname: SealedBytes::public(
                                "Member".to_string().into_bytes(),
                            ),
                        };
                        AuthorizedMemberInfo::new_with_member_key(member_info, &board_data.self_sk)
                    };

                    (
                        Some(river_core::board_state::member::MembersDelta::new(
                            members_to_add,
                        )),
                        Some(vec![authorized_info]),
                    )
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        }
    };

    let delta = ChatBoardStateV1Delta {
        recent_messages: Some(vec![auth_message.clone()]),
        members: members_delta,
        member_info: member_info_delta,
        ..Default::default()
    };

    info!("Sending message: {:?}", auth_message);

    BOARDS.with_mut(|boards| {
        if let Some(board_data) = boards.map.get_mut(&current_board) {
            if let Err(e) = board_data.board_state.apply_delta(
                &board_state_clone,
                &ChatBoardParametersV1 {
                    owner: current_board,
                },
                &Some(delta),
            ) {
                error!("Failed to apply message delta: {:?}", e);
            } else {
                // Mark board as needing sync after message added (deferred to avoid RefCell panics)
                mark_needs_sync(current_board);

                // Request notification permission on first message
                request_permission_on_first_message();
            }
        }
    });
}

/// Send a category message to the current board.
///
/// Only board owners and admins can create categories.
pub async fn send_category(
    name: String,
    description: Option<String>,
    icon: Option<String>,
    color: Option<String>,
    parent_category_id: Option<MessageId>,
) -> Result<(), String> {
    use crate::components::app::CURRENT_BOARD;

    // Get required board data
    let (current_board, board_key, self_sk, board_state_clone, can_create) = {
        let current_board_read = CURRENT_BOARD.read();
        let boards_read = BOARDS.read();

        let current_board = current_board_read
            .owner_key
            .ok_or("No board selected")?;

        let board_data = boards_read
            .map
            .get(&current_board)
            .ok_or("Board not found")?;

        let can_create = board_data.can_create_categories();

        (
            current_board,
            board_data.board_key(),
            board_data.self_sk.clone(),
            board_data.board_state.clone(),
            can_create,
        )
    };

    if !can_create {
        return Err("Only board owners and admins can create categories".to_string());
    }

    // Create the category content
    let content = BoardMessageBody::category(name, description, icon, color, parent_category_id);

    let message = MessageV1 {
        board_owner: MemberId::from(current_board),
        author: MemberId::from(&self_sk.verifying_key()),
        content,
        time: get_current_system_time(),
    };

    // Serialize message to CBOR for signing
    let mut message_bytes = Vec::new();
    ciborium::ser::into_writer(&message, &mut message_bytes)
        .map_err(|e| format!("Failed to serialize message: {:?}", e))?;

    // Sign using delegate with fallback to local signing
    let signature =
        crate::signing::sign_message_with_fallback(board_key, message_bytes, &self_sk).await;

    let auth_message = AuthorizedMessageV1::with_signature(message, signature);

    let delta = ChatBoardStateV1Delta {
        recent_messages: Some(vec![auth_message.clone()]),
        ..Default::default()
    };

    info!("Creating category: {:?}", auth_message);

    // Apply the delta
    BOARDS.with_mut(|boards| {
        if let Some(board_data) = boards.map.get_mut(&current_board) {
            if let Err(e) = board_data.board_state.apply_delta(
                &board_state_clone,
                &ChatBoardParametersV1 {
                    owner: current_board,
                },
                &Some(delta),
            ) {
                error!("Failed to apply category delta: {:?}", e);
            } else {
                // Mark board as needing sync
                mark_needs_sync(current_board);
            }
        }
    });

    Ok(())
}
