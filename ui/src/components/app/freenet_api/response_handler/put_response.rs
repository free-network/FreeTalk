use crate::components::app::freenet_api::board_synchronizer::BoardSynchronizer;
use crate::components::app::freenet_api::error::SynchronizerError;
use crate::components::app::sync_info::{BoardSyncStatus, SYNC_INFO};
use crate::components::app::BOARDS;
use crate::util::owner_vk_to_contract_key;
use dioxus::logger::tracing::{error, info, warn};
use dioxus::prelude::ReadableExt;
use freenet_stdlib::prelude::ContractKey;
use river_core::board_state::member::MemberId;

pub async fn handle_put_response(
    board_synchronizer: &mut BoardSynchronizer,
    key: ContractKey,
) -> Result<(), SynchronizerError> {
    let contract_id = key.id();
    info!("Received PutResponse for contract ID: {}", contract_id);

    // Get the owner VK first, then release the read lock
    let owner_vk_opt = {
        let sync_info = SYNC_INFO.read();
        sync_info.get_owner_vk_for_instance_id(contract_id)
    };

    // If not found in SYNC_INFO, try fallback lookup from boards
    // This handles the case where the board creator's SYNC_INFO wasn't properly initialized
    let owner_vk_opt = if owner_vk_opt.is_none() {
        warn!(
            "Owner VK not found in SYNC_INFO for contract ID: {}, trying fallback from boards",
            contract_id
        );

        let boards = BOARDS.read();
        let mut found_owner_vk = None;

        for owner_key in boards.map.keys() {
            let board_contract_key = owner_vk_to_contract_key(owner_key);
            if board_contract_key.id() == contract_id {
                info!(
                    "Found matching owner key in boards: {:?}",
                    MemberId::from(*owner_key)
                );
                found_owner_vk = Some(*owner_key);
                break;
            }
        }

        found_owner_vk
    } else {
        owner_vk_opt
    };

    match owner_vk_opt {
        Some(owner_vk) => {
            info!(
                "Found owner VK for contract ID {}: {:?}",
                contract_id,
                MemberId::from(owner_vk)
            );

            // Register board in SYNC_INFO (deferred — subscribe doesn't read SYNC_INFO)
            crate::util::defer(move || {
                SYNC_INFO.with_mut(|sync_info| {
                    sync_info.register_new_board(owner_vk);
                });
            });

            // Now subscribe to the contract
            let subscribe_result = board_synchronizer.subscribe_to_contract(&key).await;

            if let Err(e) = subscribe_result {
                error!("Failed to subscribe to contract after PUT: {}", e);
                let error_msg = e.to_string();
                crate::util::defer(move || {
                    SYNC_INFO
                        .write()
                        .update_sync_status(&owner_vk, BoardSyncStatus::Error(error_msg));
                });
            } else {
                crate::util::defer(move || {
                    SYNC_INFO
                        .write()
                        .update_sync_status(&owner_vk, BoardSyncStatus::Subscribed);
                });
            }

            // Log the current state of all boards after successful PUT
            let boards_count = {
                let boards = BOARDS.read();
                boards.map.len()
            };
            info!("Current boards count after PutResponse: {}", boards_count);

            // Get board information in a separate block
            let board_info: Vec<(MemberId, String)> = {
                let boards = BOARDS.read();
                boards
                    .map
                    .keys()
                    .map(|board_key| {
                        let contract_key = owner_vk_to_contract_key(board_key);
                        let board_contract_id = contract_key.id();
                        (MemberId::from(*board_key), board_contract_id.to_string())
                    })
                    .collect()
            };

            // Log board information
            for (member_id, contract_id) in board_info {
                info!(
                    "Board in map: {:?}, contract ID: {}",
                    member_id, contract_id
                );
            }
        }
        None => {
            warn!(
                "Warning: Could not find owner VK for contract ID: {}",
                contract_id
            );
        }
    }

    Ok(())
}
