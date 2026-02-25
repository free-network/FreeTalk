#![allow(dead_code)]

use super::error::SynchronizerError;
use crate::components::app::chat_delegate::save_boards_to_delegate;
use crate::components::app::document_title::{
    mark_current_board_as_read, update_document_title, DOCUMENT_VISIBLE,
};
use crate::components::app::freenet_api::constants::INVITATION_TIMEOUT_MS;
use crate::components::app::notifications::{mark_initial_sync_complete, notify_new_messages};
use crate::components::app::receive_times::record_receive_times;
use crate::components::app::sync_info::{now_ms, BoardSyncStatus, SYNC_INFO};
use crate::components::app::{CURRENT_BOARD, PENDING_INVITES, BOARDS, WEB_API};
use crate::constants::BOARD_CONTRACT_WASM;
use crate::invites::PendingBoardStatus;
use crate::util::ecies::decrypt_with_symmetric_key;
use crate::util::{owner_vk_to_contract_key, to_cbor_vec};
use dioxus::logger::tracing::{error, info, warn};
use dioxus::prelude::*;
use ed25519_dalek::VerifyingKey;
use freenet_scaffold::ComposableState;
use freenet_stdlib::{
    client_api::{ClientRequest, ContractRequest},
    prelude::{
        ContractCode, ContractContainer, ContractInstanceId, ContractKey, ContractWasmAPIVersion,
        Parameters, UpdateData, WrappedContract, WrappedState,
    },
};
use river_core::board_state::member::MemberId;
use river_core::board_state::message::{MessageId, BoardMessageBody};
use river_core::board_state::privacy::PrivacyMode;
use river_core::board_state::{ChatBoardParametersV1, ChatBoardStateV1, ChatBoardStateV1Delta};
use std::collections::HashMap;
use std::sync::Arc;

/// Identifies contracts that have changed in order to send state updates to Freene
#[derive(Clone)]
pub struct BoardSynchronizer {
    contract_sync_info: HashMap<ContractInstanceId, ContractSyncInfo>,
}

impl BoardSynchronizer {
    pub(crate) fn apply_delta(&self, owner_vk: &VerifyingKey, delta: ChatBoardStateV1Delta) {
        // Extract new messages for notifications before entering the mutable borrow
        let new_messages = delta.recent_messages.clone();

        BOARDS.with_mut(|boards| {
            if let Some(board_data) = boards.map.get_mut(owner_vk) {
                let params = ChatBoardParametersV1 { owner: *owner_vk };

                // Log the delta being applied, especially any member_info with versions
                if let Some(member_info) = &delta.member_info {
                    info!("Applying member_info delta with {} items", member_info.len());
                    for info in member_info {
                        info!("Delta contains member_info with version: {} for member: {:?}, nickname: {}",
                              info.member_info.version,
                              info.member_info.member_id,
                              info.member_info.preferred_nickname);
                    }
                }

                // Log current versions before applying delta
                info!("Current member_info state before delta ({} items):",
                      board_data.board_state.member_info.member_info.len());
                for info in &board_data.board_state.member_info.member_info {
                    info!("Current member_info version: {} for member: {:?}, nickname: {}",
                          info.member_info.version,
                          info.member_info.member_id,
                          info.member_info.preferred_nickname);
                }

                // Capture data for notifications before we modify board_data
                let self_member_id: MemberId = board_data.self_sk.verifying_key().into();
                let member_info = board_data.board_state.member_info.clone();
                let board_secrets = board_data.secrets.clone();

                // Clone the state to avoid borrowing issues
                let state_clone = board_data.board_state.clone();

                match board_data
                    .board_state
                    .apply_delta(&state_clone, &params, &Some(delta))
                {
                    Ok(_) => {
                        // For private boards, rebuild actions_state with decrypted content
                        // (apply_delta only processes public actions)
                        let is_private = board_data.board_state.configuration.configuration.privacy_mode
                            == PrivacyMode::Private;
                        if is_private {
                            // Decrypt all private action messages using version-aware lookup
                            let decrypted_actions: HashMap<MessageId, Vec<u8>> = board_data
                                .board_state
                                .recent_messages
                                .messages
                                .iter()
                                .filter(|msg| msg.message.content.is_action())
                                .filter_map(|msg| {
                                    if let BoardMessageBody::Private { ciphertext, nonce, secret_version, .. } =
                                        &msg.message.content
                                    {
                                        board_data.get_secret_for_version(*secret_version)
                                            .and_then(|secret| {
                                                decrypt_with_symmetric_key(secret, ciphertext, nonce)
                                                    .ok()
                                                    .map(|plaintext| (msg.id(), plaintext))
                                            })
                                    } else {
                                        None
                                    }
                                })
                                .collect();

                            board_data
                                .board_state
                                .recent_messages
                                .rebuild_actions_state_with_decrypted(&decrypted_actions);
                        }

                        // Log versions after applying delta
                        info!("Updated member_info state after delta ({} items):",
                              board_data.board_state.member_info.member_info.len());
                        for info in &board_data.board_state.member_info.member_info {
                            info!("Updated member_info version: {} for member: {:?}, nickname: {}",
                                  info.member_info.version,
                                  info.member_info.member_id,
                                  info.member_info.preferred_nickname);
                        }

                        // Keep cached self membership data up to date
                        board_data.capture_self_membership_data(&params);

                        // Update the last synced state
                        SYNC_INFO
                            .write()
                            .update_last_synced_state(owner_vk, &board_data.board_state);

                        // Notify about new messages from other users
                        if let Some(messages) = new_messages {
                            // Record receive timestamps for propagation delay tracking
                            let msg_ids: Vec<_> = messages.iter().map(|m| m.id()).collect();
                            record_receive_times(&msg_ids);

                            notify_new_messages(
                                owner_vk,
                                &messages,
                                self_member_id,
                                &member_info,
                                &board_secrets,
                            );

                            // If user is viewing this board with tab visible, mark as read immediately
                            let is_visible = *DOCUMENT_VISIBLE.read();
                            let is_current_board = CURRENT_BOARD.read().owner_key == Some(*owner_vk);
                            if is_visible && is_current_board {
                                mark_current_board_as_read();
                            }
                        }

                        // Update document title (may show unread count)
                        update_document_title();

                        // Persist to delegate so state survives refresh
                        wasm_bindgen_futures::spawn_local(async {
                            if let Err(e) = save_boards_to_delegate().await {
                                error!("Failed to save boards to delegate after delta: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Failed to apply delta: {}", e);
                    }
                }
            } else {
                warn!("Board not found in boards map for apply_delta, ignoring delta");
                // For now, we'll just ignore deltas for boards we don't have
                // The board should be created through a GET response, not a delta
            }
        });
    }
}

impl BoardSynchronizer {
    pub fn new() -> Self {
        Self {
            contract_sync_info: HashMap::new(),
        }
    }

    /// Send updates to the network for any board that has changed locally
    /// Should be called after modification detected to Signal<Boards>
    pub async fn process_boards(&mut self) -> Result<(), SynchronizerError> {
        info!("Processing boards");

        // Check if WebAPI is available before processing invitations
        // This prevents updating status when we can't actually send requests
        let web_api_available = WEB_API.read().is_some();

        // Reset stuck invitations that have been in Subscribing state too long
        if web_api_available {
            let stuck_invites: Vec<VerifyingKey> = {
                let pending = PENDING_INVITES.read();
                let now = now_ms();
                pending
                    .map
                    .iter()
                    .filter(|(_, join)| {
                        matches!(join.status, PendingBoardStatus::Subscribing)
                            && join
                                .subscribing_since
                                .is_none_or(|since| now - since > INVITATION_TIMEOUT_MS as f64)
                    })
                    .map(|(vk, _)| *vk)
                    .collect()
            };
            for vk in stuck_invites {
                warn!(
                    "Invitation for {:?} stuck in Subscribing, resetting for retry",
                    MemberId::from(vk)
                );
                PENDING_INVITES.with_mut(|pending| {
                    if let Some(join) = pending.map.get_mut(&vk) {
                        join.status = PendingBoardStatus::PendingSubscription;
                        join.subscribing_since = None;
                    }
                });
                SYNC_INFO
                    .write()
                    .update_sync_status(&vk, BoardSyncStatus::Disconnected);
            }
        }

        // First, check for pending invitations that need subscription
        // Collect keys that need subscription without holding the read lock
        let invites_to_subscribe: Vec<VerifyingKey> = if web_api_available {
            let pending_invites = PENDING_INVITES.read();
            pending_invites
                .map
                .iter()
                .filter(|(_, join)| matches!(join.status, PendingBoardStatus::PendingSubscription))
                .map(|(key, _)| *key)
                .collect()
        } else {
            // WebAPI not available, skip invitation processing until connection established
            Vec::new()
        };

        if !invites_to_subscribe.is_empty() {
            info!(
                "Found {} pending invitations to subscribe to",
                invites_to_subscribe.len()
            );

            for owner_vk in invites_to_subscribe {
                info!(
                    "Subscribing to board for invitation: {:?}",
                    MemberId::from(owner_vk)
                );

                let contract_key = owner_vk_to_contract_key(&owner_vk);

                // Register the board in SYNC_INFO and update pending invite status atomically
                // This ensures the contract ID is associated with the owner_vk
                // when the response comes back, and prevents re-processing on retry
                info!(
                    "Registering board in SYNC_INFO for owner: {:?}, contract ID: {}",
                    MemberId::from(owner_vk),
                    contract_key.id()
                );

                // Use with_mut to scope the borrow properly and avoid AlreadyBorrowed errors
                SYNC_INFO.with_mut(|sync_info| {
                    sync_info.register_new_board(owner_vk);
                    sync_info.update_sync_status(&owner_vk, BoardSyncStatus::Subscribing);
                });

                // Update pending invite status to prevent re-processing on concurrent calls
                PENDING_INVITES.with_mut(|pending| {
                    if let Some(join) = pending.map.get_mut(&owner_vk) {
                        join.status = PendingBoardStatus::Subscribing;
                        join.subscribing_since = Some(now_ms());
                    }
                });

                // Create a get request without subscription (will subscribe after response)
                let get_request = ContractRequest::Get {
                    key: *contract_key.id(),    // GET uses ContractInstanceId
                    return_contract_code: true, // I think this should be false but apparently that was triggering a bug
                    subscribe: false,
                    blocking_subscribe: false,
                };

                let client_request = ClientRequest::ContractOp(get_request);

                // WebAPI availability was checked at the start of this function
                if let Some(web_api) = WEB_API.write().as_mut() {
                    match web_api.send(client_request).await {
                        Ok(_) => {
                            info!("Sent GetRequest for board {:?}", MemberId::from(owner_vk));
                        }
                        Err(e) => {
                            error!(
                                "Error sending GetRequest to board {:?}: {}",
                                MemberId::from(owner_vk),
                                e
                            );
                            // Update pending invite status to error
                            PENDING_INVITES.with_mut(|pending| {
                                if let Some(join) = pending.map.get_mut(&owner_vk) {
                                    join.status = PendingBoardStatus::Error(e.to_string());
                                }
                            });
                        }
                    }
                } else {
                    // This shouldn't happen since we checked at the start, but handle gracefully
                    warn!("WebAPI became unavailable during processing, resetting status");
                    PENDING_INVITES.with_mut(|pending| {
                        if let Some(join) = pending.map.get_mut(&owner_vk) {
                            join.status = PendingBoardStatus::PendingSubscription;
                        }
                    });
                }
            }
        }

        info!("Checking for boards that need to be subscribed");

        // Only check boards_awaiting_subscription if WebAPI is available
        let boards_to_subscribe = if web_api_available {
            SYNC_INFO.with_mut(|sync_info| sync_info.boards_awaiting_subscription())
        } else {
            std::collections::HashMap::new()
        };

        if !boards_to_subscribe.is_empty() {
            for (owner_vk, state) in &boards_to_subscribe {
                info!("Subscribing to board: {:?}", MemberId::from(*owner_vk));

                let contract_code = ContractCode::from(BOARD_CONTRACT_WASM);
                let parameters = ChatBoardParametersV1 { owner: *owner_vk };
                let params_bytes = to_cbor_vec(&parameters);
                let parameters = Parameters::from(params_bytes);

                let contract_container = ContractContainer::from(ContractWasmAPIVersion::V1(
                    WrappedContract::new(Arc::new(contract_code), parameters),
                ));

                let wrapped_state = WrappedState::new(to_cbor_vec(state));

                // Create a put request without subscription (will subscribe after response)
                let contract_key = owner_vk_to_contract_key(owner_vk);
                let contract_id = contract_key.id();
                info!(
                    "Preparing PutRequest for board {:?} with contract ID: {}",
                    MemberId::from(*owner_vk),
                    contract_id
                );

                let put_request = ContractRequest::Put {
                    contract: contract_container,
                    state: wrapped_state,
                    related_contracts: Default::default(),
                    subscribe: true,
                    blocking_subscribe: false,
                };

                let client_request = ClientRequest::ContractOp(put_request);

                info!(
                    "Sending PutRequest for board {:?} with contract ID: {}",
                    MemberId::from(*owner_vk),
                    contract_id
                );

                if let Some(web_api) = WEB_API.write().as_mut() {
                    match web_api.send(client_request).await {
                        Ok(_) => {
                            info!("Sent PutRequest for board {:?}", MemberId::from(*owner_vk));
                            // Update the sync status to subscribing using with_mut
                            SYNC_INFO.with_mut(|sync_info| {
                                sync_info.update_sync_status(owner_vk, BoardSyncStatus::Subscribing);
                            });
                        }
                        Err(e) => {
                            // Don't fail the entire process if one board fails
                            error!(
                                "Error sending PutRequest to board {:?}: {}",
                                MemberId::from(*owner_vk),
                                e
                            );
                            // Update sync status to error using with_mut
                            SYNC_INFO.with_mut(|sync_info| {
                                sync_info.update_sync_status(
                                    owner_vk,
                                    BoardSyncStatus::Error(e.to_string()),
                                );
                            });
                        }
                    }
                } else {
                    // This shouldn't happen since we checked at the start
                    warn!("WebAPI became unavailable during processing");
                }
            }
        }

        // Send upgrade pointers for migrated boards (owner only)
        if web_api_available {
            let migrated_BOARDS: Vec<(VerifyingKey, freenet_stdlib::prelude::ContractKey)> =
                BOARDS.with_mut(|boards| std::mem::take(&mut boards.migrated_boards));

            for (owner_vk, old_contract_key) in &migrated_BOARDS {
                // Only the board owner should send the upgrade pointer
                let is_owner = BOARDS
                    .read()
                    .map
                    .get(owner_vk)
                    .is_some_and(|rd| rd.self_sk.verifying_key() == *owner_vk);

                if !is_owner {
                    continue;
                }

                info!(
                    "Sending upgrade pointer for migrated board {:?} from old contract {} to new contract",
                    MemberId::from(*owner_vk),
                    old_contract_key.id()
                );

                // Build the upgrade state
                let (upgrade_state, _new_contract_key) = {
                    let boards = BOARDS.read();
                    if let Some(board_data) = boards.map.get(owner_vk) {
                        use river_core::board_state::upgrade::{
                            AuthorizedUpgradeV1, OptionalUpgradeV1, UpgradeV1,
                        };

                        let new_contract_id = board_data.contract_key.id();
                        let mut id_bytes = [0u8; 32];
                        id_bytes.copy_from_slice(new_contract_id.as_bytes());
                        let new_address = blake3::Hash::from(id_bytes);
                        let upgrade = UpgradeV1 {
                            owner_member_id: board_data.owner_id(),
                            version: 1,
                            new_chatboard_address: new_address,
                        };
                        let authorized_upgrade =
                            AuthorizedUpgradeV1::new(upgrade, &board_data.self_sk);

                        // Create a minimal state with just the upgrade field set
                        let upgrade_state = ChatBoardStateV1 {
                            upgrade: OptionalUpgradeV1(Some(authorized_upgrade)),
                            ..Default::default()
                        };

                        (upgrade_state, board_data.contract_key)
                    } else {
                        continue;
                    }
                };

                let update_request = ContractRequest::Update {
                    key: *old_contract_key,
                    data: UpdateData::State(to_cbor_vec(&upgrade_state).into()),
                };

                let client_request = ClientRequest::ContractOp(update_request);

                if let Some(web_api) = WEB_API.write().as_mut() {
                    match web_api.send(client_request).await {
                        Ok(_) => {
                            info!(
                                "Sent upgrade pointer for board {:?} to old contract {}",
                                MemberId::from(*owner_vk),
                                old_contract_key.id()
                            );
                        }
                        Err(e) => {
                            warn!(
                                "Failed to send upgrade pointer for board {:?}: {}",
                                MemberId::from(*owner_vk),
                                e
                            );
                        }
                    }
                }
            }
        }

        info!("Checking for boards to update");

        // Only check for boards needing updates if WebAPI is available
        let boards_to_sync = if web_api_available {
            SYNC_INFO.with_mut(|sync_info| sync_info.needs_to_send_update())
        } else {
            std::collections::HashMap::new()
        };

        info!(
            "Found {} boards that need synchronization",
            boards_to_sync.len()
        );

        for (board_vk, state) in &boards_to_sync {
            info!("Processing board: {:?}", MemberId::from(*board_vk));

            let contract_key = owner_vk_to_contract_key(board_vk);

            let update_request = ContractRequest::Update {
                key: contract_key,
                data: UpdateData::State(to_cbor_vec(state).into()),
            };

            let client_request = ClientRequest::ContractOp(update_request);

            if let Some(web_api) = WEB_API.write().as_mut() {
                match web_api.send(client_request).await {
                    Ok(_) => {
                        info!(
                            "Successfully sent update for board: {:?}",
                            MemberId::from(*board_vk)
                        );
                        // Only update the last synced state after successfully sending the update
                        SYNC_INFO.with_mut(|sync_info| {
                            sync_info.state_updated(board_vk, state.clone());
                        });
                    }
                    Err(e) => {
                        // Don't fail the entire process if one board fails
                        error!(
                            "Failed to send update for board {:?}: {}",
                            MemberId::from(*board_vk),
                            e
                        );
                    }
                }
            } else {
                // This shouldn't happen since we checked at the start
                warn!("WebAPI became unavailable during processing");
            }
        }

        info!("Finished processing all boards");

        Ok(())
    }

    /// Updates the board state and last_sync_state, should be called after state update received from network
    pub(crate) fn update_board_state(&self, board_owner_vk: &VerifyingKey, state: &ChatBoardStateV1) {
        // Capture data needed for notifications BEFORE the mutable borrow
        let (old_message_ids, self_member_id, member_info_clone, board_secrets) = {
            let boards = BOARDS.read();
            if let Some(board_data) = boards.map.get(board_owner_vk) {
                let old_ids: std::collections::HashSet<_> = board_data
                    .board_state
                    .recent_messages
                    .messages
                    .iter()
                    .map(|m| m.id())
                    .collect();
                info!(
                    "update_board_state: Captured {} old message IDs for board {:?}",
                    old_ids.len(),
                    MemberId::from(*board_owner_vk)
                );
                let self_id = MemberId::from(&board_data.self_sk.verifying_key());
                let member_info = board_data.board_state.member_info.clone();
                let secrets = board_data.secrets.clone();
                (Some(old_ids), Some(self_id), Some(member_info), secrets)
            } else {
                info!(
                    "update_board_state: Board {:?} not found in boards when capturing old IDs",
                    MemberId::from(*board_owner_vk)
                );
                (None, None, None, HashMap::new())
            }
        };

        // Log incoming state message count
        info!(
            "update_board_state: Incoming state has {} messages for board {:?}",
            state.recent_messages.messages.len(),
            MemberId::from(*board_owner_vk)
        );

        // Will be populated inside with_mut if new messages are detected
        let mut pending_notification: Option<(Vec<_>, MemberId)> = None;
        let board_owner_copy = *board_owner_vk;

        BOARDS.with_mut(|boards| {
            if let Some(board_data) = boards.map.get_mut(board_owner_vk) {
                // Log member info versions before merge
                info!(
                    "Before merge - Local member info versions ({} items):",
                    board_data.board_state.member_info.member_info.len()
                );
                for info in &board_data.board_state.member_info.member_info {
                    info!(
                        "  Member: {:?}, Version: {}, Nickname: {}",
                        info.member_info.member_id,
                        info.member_info.version,
                        info.member_info.preferred_nickname
                    );
                }

                info!(
                    "Before merge - Incoming state member info versions ({} items):",
                    state.member_info.member_info.len()
                );
                for info in &state.member_info.member_info {
                    info!(
                        "  Member: {:?}, Version: {}, Nickname: {}",
                        info.member_info.member_id,
                        info.member_info.version,
                        info.member_info.preferred_nickname
                    );
                }

                // Update the board state by merging the new state with the existing one
                match board_data.board_state.merge(
                    &board_data.board_state.clone(),
                    &ChatBoardParametersV1 {
                        owner: *board_owner_vk,
                    },
                    state,
                ) {
                    Ok(_) => {
                        // For private boards, rebuild actions_state with decrypted content
                        let is_private = board_data.board_state.configuration.configuration.privacy_mode
                            == PrivacyMode::Private;
                        if is_private {
                            // Decrypt all private action messages using version-aware lookup
                            let decrypted_actions: HashMap<MessageId, Vec<u8>> = board_data
                                .board_state
                                .recent_messages
                                .messages
                                .iter()
                                .filter(|msg| msg.message.content.is_action())
                                .filter_map(|msg| {
                                    if let BoardMessageBody::Private { ciphertext, nonce, secret_version, .. } =
                                        &msg.message.content
                                    {
                                        board_data.get_secret_for_version(*secret_version)
                                            .and_then(|secret| {
                                                decrypt_with_symmetric_key(secret, ciphertext, nonce)
                                                    .ok()
                                                    .map(|plaintext| (msg.id(), plaintext))
                                            })
                                    } else {
                                        None
                                    }
                                })
                                .collect();

                            board_data
                                .board_state
                                .recent_messages
                                .rebuild_actions_state_with_decrypted(&decrypted_actions);
                        }

                        // Log member info versions after merge
                        info!(
                            "After merge - Updated member info versions ({} items):",
                            board_data.board_state.member_info.member_info.len()
                        );
                        for info in &board_data.board_state.member_info.member_info {
                            info!(
                                "  Member: {:?}, Version: {}, Nickname: {}",
                                info.member_info.member_id,
                                info.member_info.version,
                                info.member_info.preferred_nickname
                            );
                        }

                        // Keep cached self membership data up to date
                        let params = ChatBoardParametersV1 { owner: *board_owner_vk };
                        board_data.capture_self_membership_data(&params);

                        // Make sure the board is registered in SYNC_INFO
                        SYNC_INFO.with_mut(|sync_info| {
                            sync_info.register_new_board(*board_owner_vk);
                            // We use the post-merged state to avoid some edge cases
                            sync_info
                                .update_last_synced_state(board_owner_vk, &board_data.board_state);
                        });

                        // Mark initial sync complete for this board (enables notifications)
                        mark_initial_sync_complete(board_owner_vk);

                        // Detect new messages - store for notification AFTER with_mut completes
                        // (notify_new_messages calls boards.read() internally, causing deadlock if called here)
                        if let (Some(old_ids), Some(self_id), Some(_member_info)) =
                            (&old_message_ids, self_member_id, &member_info_clone)
                        {
                            let new_messages: Vec<_> = board_data
                                .board_state
                                .recent_messages
                                .messages
                                .iter()
                                .filter(|m| !old_ids.contains(&m.id()))
                                .cloned()
                                .collect();

                            if !new_messages.is_empty() {
                                info!(
                                    "Detected {} new messages in state update for board {:?}",
                                    new_messages.len(),
                                    MemberId::from(*board_owner_vk)
                                );

                                // Note: we do NOT record receive times here. Full state
                                // updates don't reflect real-time arrival — we don't know
                                // when these messages actually propagated to our node.
                                // Only apply_delta (subscription deltas) captures the
                                // true arrival moment.

                                // Store for notification after with_mut completes
                                pending_notification = Some((new_messages, self_id));
                            } else {
                                info!(
                                    "No new messages detected for board {:?} (old_ids: {}, post-merge: {})",
                                    MemberId::from(*board_owner_vk),
                                    old_ids.len(),
                                    board_data.board_state.recent_messages.messages.len()
                                );
                            }
                        }

                        // Persist to delegate so state survives refresh
                        wasm_bindgen_futures::spawn_local(async {
                            if let Err(e) = save_boards_to_delegate().await {
                                error!("Failed to save boards to delegate after state update: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Failed to merge board state: {}", e);
                    }
                }
            } else {
                warn!("Board not found in boards map for update_board_state. This can happen if we receive an update before the board is fully initialized.");
                // We cannot create a board here because we don't have the self_sk (signing key)
                // Instead, we should request the full state with a GET reques
                // This is handled by registering the board in SYNC_INFO which will trigger a GET request in the next sync cycle

                // Register the board in SYNC_INFO to trigger a GET reques
                SYNC_INFO.with_mut(|sync_info| {
                    sync_info.register_new_board(*board_owner_vk);
                    // Store the state temporarily so it can be merged when we get the full board data
                    sync_info.update_last_synced_state(board_owner_vk, state);
                });

                info!("Registered board {:?} for GET request after receiving update without existing board data", MemberId::from(*board_owner_vk));
            }
        });

        // Update document title after boards.with_mut completes (update_document_title calls boards.read())
        update_document_title();

        // Now safe to call notify_new_messages (it calls boards.read() internally)
        if let (Some((new_messages, self_id)), Some(member_info)) =
            (pending_notification, member_info_clone)
        {
            notify_new_messages(
                &board_owner_copy,
                &new_messages,
                self_id,
                &member_info,
                &board_secrets,
            );

            // If user is viewing this board with tab visible, mark as read
            let is_visible = *DOCUMENT_VISIBLE.read();
            let is_current_board = CURRENT_BOARD.read().owner_key == Some(board_owner_copy);
            if is_visible && is_current_board {
                mark_current_board_as_read();
            }
        }
    }

    /// Refresh all board states by sending GET requests.
    /// This is used after PC suspension/wake to catch any updates that were missed
    /// while the page was hidden or the machine was suspended.
    pub async fn refresh_all_boards(&self) -> Result<(), SynchronizerError> {
        info!("Refreshing all boards to catch missed updates");

        // Check if WebAPI is available
        let web_api_available = WEB_API.read().is_some();
        if !web_api_available {
            warn!("WebAPI not available, skipping board refresh");
            return Err(SynchronizerError::ApiNotInitialized);
        }

        // Collect all board owner keys that we're currently tracking
        let board_owners: Vec<VerifyingKey> = BOARDS.read().map.keys().copied().collect();

        if board_owners.is_empty() {
            info!("No boards to refresh");
            return Ok(());
        }

        info!("Refreshing {} boards", board_owners.len());

        for owner_vk in board_owners {
            let contract_key = owner_vk_to_contract_key(&owner_vk);

            // Send a GET request to fetch the current state
            // This will trigger a response that merges any missed updates
            let get_request = ContractRequest::Get {
                key: *contract_key.id(),
                return_contract_code: false,
                subscribe: false, // Already subscribed, just need the state
                blocking_subscribe: false,
            };

            let client_request = ClientRequest::ContractOp(get_request);

            if let Some(web_api) = WEB_API.write().as_mut() {
                match web_api.send(client_request).await {
                    Ok(_) => {
                        info!(
                            "Sent refresh GET request for board {:?}",
                            MemberId::from(owner_vk)
                        );
                    }
                    Err(e) => {
                        // Don't fail the entire refresh if one board fails
                        error!(
                            "Error sending refresh GET for board {:?}: {}",
                            MemberId::from(owner_vk),
                            e
                        );
                    }
                }
            } else {
                warn!("WebAPI became unavailable during refresh");
                return Err(SynchronizerError::ApiNotInitialized);
            }
        }

        info!("Finished sending refresh requests for all boards");
        Ok(())
    }

    /// Fetch the current state of a contract via GET request.
    /// Used after successful subscribe to ensure we have the latest state,
    /// since delegate storage may contain stale data from a previous session.
    pub async fn get_contract_state(
        &self,
        contract_key: &ContractKey,
    ) -> Result<(), SynchronizerError> {
        info!("Fetching current state for contract: {}", contract_key.id());

        let get_request = ContractRequest::Get {
            key: *contract_key.id(),
            return_contract_code: false,
            subscribe: false,
            blocking_subscribe: false,
        };

        let client_request = ClientRequest::ContractOp(get_request);

        if let Some(web_api) = WEB_API.write().as_mut() {
            match web_api.send(client_request).await {
                Ok(_) => {
                    info!("Sent GET request for contract: {}", contract_key.id());
                    Ok(())
                }
                Err(e) => {
                    error!("Failed to send GET request for contract: {}", e);
                    Err(SynchronizerError::ClientApiError(e.to_string()))
                }
            }
        } else {
            warn!("WebAPI not available for GET request");
            Err(SynchronizerError::ApiNotInitialized)
        }
    }

    /// Subscribe to a contract after a successful GET or PUT operation
    pub async fn subscribe_to_contract(
        &self,
        contract_key: &ContractKey,
    ) -> Result<(), SynchronizerError> {
        info!("Subscribing to contract with key: {}", contract_key.id());

        let subscribe_request = ContractRequest::Subscribe {
            key: *contract_key.id(), // Subscribe uses ContractInstanceId
            summary: None,
        };

        let client_request = ClientRequest::ContractOp(subscribe_request);

        if let Some(web_api) = WEB_API.write().as_mut() {
            match web_api.send(client_request).await {
                Ok(_) => {
                    info!(
                        "Successfully sent subscription request for contract: {}",
                        contract_key.id()
                    );
                    Ok(())
                }
                Err(e) => {
                    error!("Failed to send subscription request: {}", e);
                    Err(SynchronizerError::SubscribeError(e.to_string()))
                }
            }
        } else {
            warn!("WebAPI not available, skipping subscription");
            Err(SynchronizerError::ApiNotInitialized)
        }
    }
}

/// Stores information about a contract being synchronized
#[derive(Clone)]
pub struct ContractSyncInfo {
    pub owner_vk: VerifyingKey,
}
