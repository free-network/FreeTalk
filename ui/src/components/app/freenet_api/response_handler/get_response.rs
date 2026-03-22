use crate::board_data::BoardData;
use crate::components::app::freenet_api::board_synchronizer::BoardSynchronizer;
use crate::components::app::freenet_api::error::SynchronizerError;
use crate::components::app::notifications::mark_initial_sync_complete;
use crate::components::app::sync_info::{BoardSyncStatus, SYNC_INFO};
use crate::components::app::{BOARDS, CURRENT_BOARD, PENDING_INVITES};
use crate::invites::PendingBoardStatus;
use crate::util::ecies::{decrypt_secret_from_member_blob, decrypt_with_symmetric_key};
use crate::util::{from_cbor_slice, owner_vk_to_contract_key};
use dioxus::logger::tracing::{error, info, warn};
use dioxus::prelude::ReadableExt;
use freenet_scaffold::ComposableState;
use freenet_stdlib::prelude::ContractKey;
use river_core::board_state::member::MemberId;
use river_core::board_state::member_info::{AuthorizedMemberInfo, MemberInfo};
use river_core::board_state::message::{BoardMessageBody, MessageId};
use river_core::board_state::privacy::{PrivacyMode, SealedBytes};
use river_core::board_state::{ChatBoardParametersV1, ChatBoardStateV1};
use std::collections::HashMap;
use x25519_dalek::PublicKey as X25519PublicKey;

pub async fn handle_get_response(
    board_synchronizer: &mut BoardSynchronizer,
    key: ContractKey,
    _contract: Vec<u8>,
    state: Vec<u8>,
) -> Result<(), SynchronizerError> {
    info!("Received get response for key {key}");

    // First try to find the owner_vk from SYNC_INFO
    let owner_vk = SYNC_INFO.read().get_owner_vk_for_instance_id(key.id());

    // If we couldn't find it in SYNC_INFO, try fallback mechanisms
    let owner_vk = if owner_vk.is_none() {
        // This is a fallback mechanism in case SYNC_INFO wasn't properly set up
        warn!(
            "Owner VK not found in SYNC_INFO for contract ID: {}, trying fallback",
            key.id()
        );

        // First try PENDING_INVITES
        let pending_invites = PENDING_INVITES.read();
        let mut found_owner_vk = None;

        for (owner_key, _) in pending_invites.map.iter() {
            let contract_key = owner_vk_to_contract_key(owner_key);
            if contract_key.id() == key.id() {
                info!(
                    "Found matching owner key in pending invites: {:?}",
                    MemberId::from(*owner_key)
                );
                found_owner_vk = Some(*owner_key);
                break;
            }
        }
        drop(pending_invites);

        // If not in pending invites, try boards (for refresh after suspension)
        if found_owner_vk.is_none() {
            let boards = BOARDS.read();
            for (owner_key, board_data) in boards.map.iter() {
                if board_data.contract_key.id() == key.id() {
                    info!(
                        "Found matching owner key in existing boards: {:?}",
                        MemberId::from(*owner_key)
                    );
                    found_owner_vk = Some(*owner_key);
                    break;
                }
            }
        }

        found_owner_vk
    } else {
        owner_vk
    };

    // Now check if this is for a pending invitation or an existing board needing refresh
    if let Some(owner_vk) = owner_vk {
        let is_pending_invite = PENDING_INVITES.read().map.contains_key(&owner_vk);
        let is_existing_board = BOARDS.read().map.contains_key(&owner_vk);

        if is_pending_invite {
            info!("This is a subscription for a pending invitation, adding state");
            let retrieved_state: ChatBoardStateV1 = from_cbor_slice::<ChatBoardStateV1>(&state);

            // Get the pending invite data once to avoid multiple reads
            let (self_sk, authorized_member, preferred_nickname) = {
                let pending_invites = PENDING_INVITES.read();
                let invite = &pending_invites.map[&owner_vk];
                (
                    invite.invitee_signing_key.clone(),
                    invite.authorized_member.clone(),
                    invite.preferred_nickname.clone(),
                )
            };

            // Prepare the member ID for checking
            let member_id: MemberId = authorized_member.member.member_vk.into();

            // Update the board data
            BOARDS.with_mut(|boards| {
                // Get the entry for this board
                let entry = boards.map.entry(owner_vk);

                // Check if this is a new entry before inserting
                let is_new_entry = matches!(entry, std::collections::hash_map::Entry::Vacant(_));

                // Insert or get the existing board data
                let board_data = entry.or_insert_with(|| {
                    // Create new board data if it doesn't exist
                    BoardData {
                        owner_vk,
                        board_state: retrieved_state.clone(),
                        self_sk: self_sk.clone(),
                        contract_key: key,
                        last_read_message_id: None,
                        secrets: std::collections::HashMap::new(),
                        current_secret_version: None,
                        last_secret_rotation: None,
                        key_migrated_to_delegate: false, // Will be checked/migrated on startup
                        self_authorized_member: None,
                        invite_chain: vec![],
                        self_member_info: None,
                    }
                });

                // If the board already existed, update self_sk and merge state
                if !is_new_entry {
                    // Only update self_sk if the user is NOT the board owner,
                    // to avoid stripping owner privileges
                    if board_data.self_sk.verifying_key() != owner_vk {
                        board_data.self_sk = self_sk.clone();
                        // Reset migration flag so the new key gets migrated
                        board_data.key_migrated_to_delegate = false;
                    }

                    // Create parameters for merge
                    let params = ChatBoardParametersV1 { owner: owner_vk };

                    // Clone current state to avoid borrow issues during merge
                    let current_state = board_data.board_state.clone();

                    // Merge the retrieved state into the existing state
                    board_data
                        .board_state
                        .merge(&current_state, &params, &retrieved_state)
                        .expect("Failed to merge board states");
                }

                // Decrypt ALL board secret versions if this is a private board
                if board_data.board_state.configuration.configuration.privacy_mode == PrivacyMode::Private {
                    let current_version = board_data.board_state.secrets.current_version;

                    // Extract encrypted secret data to avoid borrow issues
                    let member_secrets: Vec<_> = board_data
                        .board_state
                        .secrets
                        .encrypted_secrets
                        .iter()
                        .filter(|s| s.secret.member_id == member_id)
                        .map(|s| (
                            s.secret.secret_version,
                            s.secret.ciphertext.clone(),
                            s.secret.nonce,
                            s.secret.sender_ephemeral_public_key,
                        ))
                        .collect();

                    if member_secrets.is_empty() {
                        warn!("No encrypted secrets found for member {:?}", member_id);
                    } else {
                        info!("Found {} encrypted secrets for member {:?}", member_secrets.len(), member_id);
                        for (version, ciphertext, nonce, ephemeral_key_bytes) in member_secrets {
                            let ephemeral_key = X25519PublicKey::from(ephemeral_key_bytes);

                            match decrypt_secret_from_member_blob(
                                &ciphertext,
                                &nonce,
                                &ephemeral_key,
                                &self_sk,
                            ) {
                                Ok(decrypted_secret) => {
                                    info!("Successfully decrypted board secret version {} for member {:?}", version, member_id);
                                    board_data.set_secret(decrypted_secret, version);
                                }
                                Err(e) => {
                                    warn!("Failed to decrypt board secret version {}: {}", version, e);
                                }
                            }
                        }
                    }

                    // Ensure current_secret_version is set to the actual current version
                    board_data.current_secret_version = Some(current_version);
                }

                // Set the member's nickname in member_info regardless of whether they were already in the board
                // This ensures the member has corresponding MemberInfo even if they were already a member
                let preferred_nickname_sealed = if board_data.board_state.configuration.configuration.privacy_mode == PrivacyMode::Private {
                    // For private boards, encrypt the nickname with the board secret
                    if let Some((secret, version)) = board_data.get_secret() {
                        use crate::util::ecies::encrypt_with_symmetric_key;
                        let (ciphertext, nonce) = encrypt_with_symmetric_key(secret, preferred_nickname.as_bytes());
                        SealedBytes::Private {
                            ciphertext,
                            nonce,
                            secret_version: version,
                            declared_len_bytes: preferred_nickname.len() as u32,
                        }
                    } else {
                        warn!("Private board but no secret available for encrypting nickname, using public");
                        SealedBytes::public(preferred_nickname.clone().into_bytes())
                    }
                } else {
                    SealedBytes::public(preferred_nickname.clone().into_bytes())
                };

                let member_info = MemberInfo {
                    member_id,
                    version: 0,
                    preferred_nickname: preferred_nickname_sealed,
                };

                let authorized_member_info =
                    AuthorizedMemberInfo::new_with_member_key(member_info.clone(), &self_sk);

                // Store membership credentials for future rejoin.
                // We do NOT apply the member to board_state here — membership
                // is published atomically with the first message to avoid
                // post_apply_cleanup pruning a member with no messages.
                board_data.self_authorized_member = Some(authorized_member.clone());
                board_data.self_member_info = Some(authorized_member_info);
                // Capture invite chain from current state
                if let Ok(chain) = board_data.board_state.members.get_invite_chain(
                    &authorized_member,
                    &ChatBoardParametersV1 { owner: owner_vk },
                ) {
                    board_data.invite_chain = chain;
                }

                // Rebuild actions_state from action messages (edit, delete, reaction)
                // This is needed because actions_state is #[serde(skip)] and not serialized
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
                                // Look up the secret for this message's version
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

                    let owner_id = river_core::board_state::member::MemberId::from(&owner_vk);
                    let admin_ids: std::collections::HashSet<_> = board_data
                        .board_state
                        .admin
                        .admins
                        .iter()
                        .map(|a| a.admin.id())
                        .collect();
                    board_data
                        .board_state
                        .recent_messages
                        .rebuild_actions_state_with_permissions(&decrypted_actions, Some(owner_id), Some(&admin_ids));
                } else {
                    // Public board - rebuild from public action messages
                    board_data
                        .board_state
                        .recent_messages
                        .rebuild_actions_state();
                }
            });

            // Make sure SYNC_INFO is properly set up for this board
            SYNC_INFO.with_mut(|sync_info| {
                // Register the board if it wasn't already registered
                sync_info.register_new_board(owner_vk);

                // DO NOT update the last_synced_state here
                // This will ensure the board is marked as needing an update in the next synchronization

                // Update the sync status
                sync_info.update_sync_status(&owner_vk, BoardSyncStatus::Subscribed);
            });

            // Now subscribe to the contract
            let subscribe_result = board_synchronizer.subscribe_to_contract(&key).await;

            if let Err(e) = subscribe_result {
                error!("Failed to subscribe to contract after GET: {}", e);
                // Update the sync status to error
                SYNC_INFO
                    .write()
                    .update_sync_status(&owner_vk, BoardSyncStatus::Error(e.to_string()));
            } else {
                // Mark the invitation as subscribed and retrieved
                PENDING_INVITES.with_mut(|pending_invites| {
                    if let Some(join) = pending_invites.map.get_mut(&owner_vk) {
                        join.status = PendingBoardStatus::Subscribed;
                    }
                });

                // Mark initial sync complete for notifications
                mark_initial_sync_complete(&owner_vk);
            }

            // Dispatch an event to notify the UI
            if let Some(window) = web_sys::window() {
                let key_hex = owner_vk
                    .as_bytes()
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<String>();
                let event = web_sys::CustomEvent::new("river-invitation-accepted").unwrap();

                // Set the detail property
                js_sys::Reflect::set(
                    &event,
                    &wasm_bindgen::JsValue::from_str("detail"),
                    &wasm_bindgen::JsValue::from_str(&key_hex),
                )
                .unwrap();

                window.dispatch_event(&event).unwrap();

                // Set the current board to the newly accepted board
                CURRENT_BOARD.with_mut(|current_board| {
                    current_board.owner_key = Some(owner_vk);
                });

                // Migrate the signing key to delegate for this new board
                let signing_key_clone = self_sk.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let board_key = owner_vk.to_bytes();
                    let migrated =
                        crate::signing::migrate_signing_key(board_key, &signing_key_clone).await;
                    if migrated {
                        // Must defer signal mutations from spawn_local to
                        // avoid RefCell already borrowed panics in Dioxus runtime
                        crate::util::defer(move || {
                            BOARDS.with_mut(|boards| {
                                if let Some(board_data) = boards.map.get_mut(&owner_vk) {
                                    board_data.key_migrated_to_delegate = true;
                                    info!("Signing key migrated to delegate for new board");
                                }
                            });
                        });
                    }
                });

                // Mark board as needing sync so it gets saved to delegate storage (deferred).
                // We do NOT trigger ProcessBoards because we haven't modified the
                // board state — membership will be published with the first message.
                use crate::components::app::mark_needs_sync;
                mark_needs_sync(owner_vk);
            }
        } else if is_existing_board {
            // This is a refresh GET for an already-subscribed board (e.g., after wake from suspension)
            info!("Processing GET response for existing board (refresh after suspension)");
            let retrieved_state: ChatBoardStateV1 = from_cbor_slice::<ChatBoardStateV1>(&state);

            BOARDS.with_mut(|boards| {
                if let Some(board_data) = boards.map.get_mut(&owner_vk) {
                    // Create parameters for merge
                    let params = ChatBoardParametersV1 { owner: owner_vk };

                    // Clone current state to avoid borrow issues during merge
                    let current_state = board_data.board_state.clone();

                    // Merge the retrieved state into the existing state
                    match board_data
                        .board_state
                        .merge(&current_state, &params, &retrieved_state)
                    {
                        Ok(_) => {
                            info!(
                                "Successfully merged refreshed state for board {:?}",
                                MemberId::from(owner_vk)
                            );
                            // Note: we intentionally do NOT record receive times here.
                            // GET responses don't reflect real-time message arrival —
                            // we don't know when these messages actually propagated
                            // to our node. Only subscription UPDATE notifications
                            // capture the true arrival moment.

                            // Migration: capture self membership data for old boards
                            board_data.capture_self_membership_data(&params);
                        }
                        Err(e) => {
                            error!(
                                "Failed to merge refreshed state for board {:?}: {}",
                                MemberId::from(owner_vk),
                                e
                            );
                        }
                    }
                }
            });

            // Update sync info to reflect we received fresh state
            SYNC_INFO.with_mut(|sync_info| {
                sync_info.update_sync_status(&owner_vk, BoardSyncStatus::Subscribed);
            });
        }
    }

    Ok(())
}
