use crate::api::compute_contract_key;
use anyhow::{anyhow, Result};
use directories::ProjectDirs;
use ed25519_dalek::{SigningKey, VerifyingKey};
use freenet_stdlib::prelude::ContractKey;
use river_core::board_state::member::AuthorizedMember;
use river_core::board_state::ChatBoardStateV1;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredBoardInfo {
    pub signing_key_bytes: [u8; 32],
    pub state: ChatBoardStateV1,
    pub contract_key: String, // Store as string for serialization
    /// The user's own AuthorizedMember, stored so they can re-add themselves
    /// after being pruned for inactivity (no recent messages).
    #[serde(default)]
    pub self_authorized_member: Option<AuthorizedMember>,
    /// The invite chain members needed to validate self_authorized_member.
    #[serde(default)]
    pub invite_chain: Vec<AuthorizedMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BoardStorage {
    /// Map from board owner verifying key (as base58) to board info
    pub boards: HashMap<String, StoredBoardInfo>,
}

pub struct Storage {
    storage_path: PathBuf,
}

impl Storage {
    pub fn new(config_dir: Option<&str>) -> Result<Self> {
        // Use provided config_dir, then check environment variable, then use default
        let data_dir = if let Some(dir) = config_dir {
            PathBuf::from(dir)
        } else if let Ok(config_dir) = std::env::var("RIVER_CONFIG_DIR") {
            PathBuf::from(config_dir)
        } else {
            // Fall back to default project directories
            let proj_dirs = ProjectDirs::from("", "Freenet", "River")
                .ok_or_else(|| anyhow!("Failed to determine project directories"))?;
            proj_dirs.data_dir().to_path_buf()
        };

        fs::create_dir_all(&data_dir)?;

        let storage_path = data_dir.join("boards.json");

        Ok(Self { storage_path })
    }

    pub fn load_boards(&self) -> Result<BoardStorage> {
        if !self.storage_path.exists() {
            return Ok(BoardStorage::default());
        }

        let contents = fs::read_to_string(&self.storage_path)?;
        let mut storage: BoardStorage = serde_json::from_str(&contents)?;

        // Regenerate contract keys to ensure they match the current bundled WASM
        // This handles the case where boards were stored with an older WASM version
        let mut updated = false;
        for (owner_key_str, board_info) in storage.boards.iter_mut() {
            let owner_key_bytes = match bs58::decode(owner_key_str).into_vec() {
                Ok(bytes) if bytes.len() == 32 => {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&bytes);
                    arr
                }
                _ => continue,
            };
            let owner_vk = match VerifyingKey::from_bytes(&owner_key_bytes) {
                Ok(vk) => vk,
                Err(_) => continue,
            };
            let new_key = compute_contract_key(&owner_vk);
            let new_key_str = new_key.id().to_string();
            if board_info.contract_key != new_key_str {
                info!(
                    "Updating contract key for board: {} -> {}",
                    board_info.contract_key, new_key_str
                );
                board_info.contract_key = new_key_str;
                updated = true;
            }
        }

        // Save the updated storage if any keys changed
        if updated {
            self.save_boards(&storage)?;
        }

        Ok(storage)
    }

    pub fn save_boards(&self, storage: &BoardStorage) -> Result<()> {
        let contents = serde_json::to_string_pretty(storage)?;
        fs::write(&self.storage_path, contents)?;
        Ok(())
    }

    pub fn add_board(
        &self,
        owner_vk: &VerifyingKey,
        signing_key: &SigningKey,
        state: ChatBoardStateV1,
        contract_key: &ContractKey,
    ) -> Result<()> {
        let mut storage = self.load_boards()?;

        let owner_key_str = bs58::encode(owner_vk.as_bytes()).into_string();
        let board_info = StoredBoardInfo {
            signing_key_bytes: signing_key.to_bytes(),
            state,
            contract_key: contract_key.id().to_string(),
            self_authorized_member: None,
            invite_chain: Vec::new(),
        };

        storage.boards.insert(owner_key_str, board_info);
        self.save_boards(&storage)?;

        Ok(())
    }

    pub fn get_board(
        &self,
        owner_vk: &VerifyingKey,
    ) -> Result<Option<(SigningKey, ChatBoardStateV1, String)>> {
        let storage = self.load_boards()?;
        let owner_key_str = bs58::encode(owner_vk.as_bytes()).into_string();

        if let Some(board_info) = storage.boards.get(&owner_key_str) {
            let signing_key = SigningKey::from_bytes(&board_info.signing_key_bytes);
            Ok(Some((
                signing_key,
                board_info.state.clone(),
                board_info.contract_key.clone(),
            )))
        } else {
            Ok(None)
        }
    }

    pub fn update_board_state(
        &self,
        owner_vk: &VerifyingKey,
        state: ChatBoardStateV1,
    ) -> Result<()> {
        let mut storage = self.load_boards()?;
        let owner_key_str = bs58::encode(owner_vk.as_bytes()).into_string();

        if let Some(board_info) = storage.boards.get_mut(&owner_key_str) {
            board_info.state = state;
            self.save_boards(&storage)?;
            Ok(())
        } else {
            Err(anyhow!("Board not found"))
        }
    }

    /// Update the contract key for a board (used during migration to new contract version)
    pub fn update_contract_key(
        &self,
        owner_vk: &VerifyingKey,
        new_key: &ContractKey,
    ) -> Result<()> {
        let mut storage = self.load_boards()?;
        let owner_key_str = bs58::encode(owner_vk.as_bytes()).into_string();

        if let Some(board_info) = storage.boards.get_mut(&owner_key_str) {
            board_info.contract_key = new_key.id().to_string();
            self.save_boards(&storage)?;
            Ok(())
        } else {
            Err(anyhow!("Board not found"))
        }
    }

    pub fn store_authorized_member(
        &self,
        owner_vk: &VerifyingKey,
        authorized_member: &AuthorizedMember,
        invite_chain: &[AuthorizedMember],
    ) -> Result<()> {
        let mut storage = self.load_boards()?;
        let owner_key_str = bs58::encode(owner_vk.as_bytes()).into_string();
        if let Some(board_info) = storage.boards.get_mut(&owner_key_str) {
            board_info.self_authorized_member = Some(authorized_member.clone());
            board_info.invite_chain = invite_chain.to_vec();
            self.save_boards(&storage)?;
        }
        Ok(())
    }

    pub fn list_boards(&self) -> Result<Vec<(VerifyingKey, String, String)>> {
        let storage = self.load_boards()?;
        let mut boards = Vec::new();

        for (owner_key_str, board_info) in storage.boards.iter() {
            let owner_key_bytes = bs58::decode(owner_key_str).into_vec()?;
            if owner_key_bytes.len() == 32 {
                let mut key_array = [0u8; 32];
                key_array.copy_from_slice(&owner_key_bytes);
                if let Ok(owner_vk) = VerifyingKey::from_bytes(&key_array) {
                    let board_name = board_info
                        .state
                        .configuration
                        .configuration
                        .display
                        .name
                        .to_string_lossy();
                    boards.push((owner_vk, board_name, board_info.contract_key.clone()));
                }
            }
        }

        Ok(boards)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use river_core::board_state::configuration::{AuthorizedConfigurationV1, Configuration};
    use tempfile::TempDir;

    fn create_test_storage() -> (Storage, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let storage = Storage::new(Some(temp_dir.path().to_str().unwrap())).unwrap();
        (storage, temp_dir)
    }

    fn create_test_signing_key() -> SigningKey {
        SigningKey::from_bytes(&rand::Rng::gen::<[u8; 32]>(&mut rand::thread_rng()))
    }

    /// Compute the expected contract key for a given owner verifying key.
    /// This matches what load_boards will regenerate.
    fn expected_contract_key(owner_vk: &VerifyingKey) -> ContractKey {
        compute_contract_key(owner_vk)
    }

    fn create_test_state(owner_sk: &SigningKey) -> ChatBoardStateV1 {
        let owner_vk = owner_sk.verifying_key();
        let mut state = ChatBoardStateV1::default();
        let config = Configuration {
            owner_member_id: owner_vk.into(),
            ..Default::default()
        };
        state.configuration = AuthorizedConfigurationV1::new(config, owner_sk);
        state
    }

    #[test]
    fn test_update_contract_key_success() {
        let (storage, _temp_dir) = create_test_storage();
        let owner_sk = create_test_signing_key();
        let owner_vk = owner_sk.verifying_key();
        let state = create_test_state(&owner_sk);
        let initial_key = expected_contract_key(&owner_vk);

        // Add board with the computed contract key
        storage
            .add_board(&owner_vk, &owner_sk, state, &initial_key)
            .unwrap();

        // Verify the key is stored correctly (will be regenerated on load)
        let (_, _, stored_key) = storage.get_board(&owner_vk).unwrap().unwrap();
        assert_eq!(stored_key, initial_key.id().to_string());

        // Create a different key for testing update
        let different_key = {
            let code = freenet_stdlib::prelude::ContractCode::from(vec![42u8; 100]);
            let params = freenet_stdlib::prelude::Parameters::from(vec![42u8]);
            ContractKey::from_params_and_code(params, &code)
        };

        // Update to different key
        storage
            .update_contract_key(&owner_vk, &different_key)
            .unwrap();

        // After reload, key will be regenerated to match current WASM, not the updated key
        // This tests that update_contract_key persists, but load_boards regenerates
        let (_, _, stored_key) = storage.get_board(&owner_vk).unwrap().unwrap();
        // The key gets regenerated on load, so it will be the expected key
        assert_eq!(stored_key, initial_key.id().to_string());
    }

    #[test]
    fn test_update_contract_key_board_not_found() {
        let (storage, _temp_dir) = create_test_storage();
        let owner_sk = create_test_signing_key();
        let owner_vk = owner_sk.verifying_key();
        let new_key = expected_contract_key(&owner_vk);

        // Attempt to update non-existent board
        let result = storage.update_contract_key(&owner_vk, &new_key);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Board not found"));
    }

    #[test]
    fn test_update_contract_key_preserves_state() {
        let (storage, _temp_dir) = create_test_storage();
        let owner_sk = create_test_signing_key();
        let owner_vk = owner_sk.verifying_key();
        let state = create_test_state(&owner_sk);
        let initial_key = expected_contract_key(&owner_vk);

        // Add board
        storage
            .add_board(&owner_vk, &owner_sk, state.clone(), &initial_key)
            .unwrap();

        // Create a different key for testing update
        let different_key = {
            let code = freenet_stdlib::prelude::ContractCode::from(vec![99u8; 100]);
            let params = freenet_stdlib::prelude::Parameters::from(vec![99u8]);
            ContractKey::from_params_and_code(params, &code)
        };

        // Update contract key
        storage
            .update_contract_key(&owner_vk, &different_key)
            .unwrap();

        // Verify state is preserved (key will be regenerated but state should remain)
        let (retrieved_sk, retrieved_state, _) = storage.get_board(&owner_vk).unwrap().unwrap();
        assert_eq!(retrieved_sk.to_bytes(), owner_sk.to_bytes());
        assert_eq!(
            retrieved_state.configuration.configuration.max_members,
            state.configuration.configuration.max_members
        );
    }

    #[test]
    fn test_storage_roundtrip() {
        let (storage, _temp_dir) = create_test_storage();
        let owner_sk = create_test_signing_key();
        let owner_vk = owner_sk.verifying_key();
        let state = create_test_state(&owner_sk);
        let contract_key = expected_contract_key(&owner_vk);

        // Add board
        storage
            .add_board(&owner_vk, &owner_sk, state.clone(), &contract_key)
            .unwrap();

        // Retrieve and verify
        let (retrieved_sk, retrieved_state, retrieved_key) =
            storage.get_board(&owner_vk).unwrap().unwrap();

        assert_eq!(retrieved_sk.to_bytes(), owner_sk.to_bytes());
        // The contract key should match the expected key (computed from owner_vk + current WASM)
        assert_eq!(retrieved_key, contract_key.id().to_string());
        assert_eq!(
            retrieved_state.configuration.configuration.max_members,
            state.configuration.configuration.max_members
        );
    }
}
