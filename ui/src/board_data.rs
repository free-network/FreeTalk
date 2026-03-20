#![allow(dead_code)]

use crate::util::ecies::encrypt_secret_for_member;
use crate::util::get_current_system_time;
use crate::{constants::BOARD_CONTRACT_WASM, util::to_cbor_vec};
use ed25519_dalek::{SigningKey, VerifyingKey};
use freenet_scaffold::ComposableState;
use freenet_stdlib::prelude::{ContractCode, ContractKey, Parameters};
use river_core::board_state::configuration::{AuthorizedConfigurationV1, Configuration};
use river_core::board_state::member::AuthorizedMember;
use river_core::board_state::member::MemberId;
use river_core::board_state::member_info::{AuthorizedMemberInfo, MemberInfo};
use river_core::board_state::message::MessageId;
use river_core::board_state::privacy::{
    BoardCipherSpec, BoardDisplayMetadata, PrivacyMode, SealedBytes,
};
use river_core::board_state::secret::{
    AuthorizedEncryptedSecretForMember, AuthorizedSecretVersionRecord, EncryptedSecretForMemberV1,
    SecretVersionRecordV1,
};
use river_core::board_state::ChatBoardParametersV1;
use river_core::chat_delegate::BoardKey;
use river_core::ChatBoardStateV1;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, PartialEq)]
pub enum SendMessageError {
    UserNotMember,
    UserBanned,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardData {
    pub owner_vk: VerifyingKey,
    pub board_state: ChatBoardStateV1,
    pub self_sk: SigningKey,
    pub contract_key: ContractKey,
    /// The last message ID that was read by the user (for unread tracking)
    /// Messages after this ID from other users are considered unread.
    /// This is persisted to delegate storage.
    #[serde(default)]
    pub last_read_message_id: Option<MessageId>,
    /// All decrypted board secrets by version (if board is private)
    /// Maps secret_version -> decrypted 32-byte secret
    #[serde(skip)]
    pub secrets: HashMap<u32, [u8; 32]>,
    /// The current (latest) secret version
    #[serde(skip)]
    pub current_secret_version: Option<u32>,
    /// When the secret was last rotated (for weekly rotation checks)
    #[serde(skip)]
    pub last_secret_rotation: Option<std::time::SystemTime>,
    /// Whether the signing key has been migrated to the delegate
    /// This is runtime state and not persisted - checked on each startup
    #[serde(skip)]
    pub key_migrated_to_delegate: bool,
    /// The user's own AuthorizedMember, stored so they can re-add themselves
    /// after being pruned for inactivity (no recent messages).
    #[serde(default)]
    pub self_authorized_member: Option<AuthorizedMember>,
    /// The invite chain members needed to validate self_authorized_member.
    /// Contains all members in the chain from self up to (but not including) the owner.
    #[serde(default)]
    pub invite_chain: Vec<AuthorizedMember>,
    /// The user's own AuthorizedMemberInfo, stored so their nickname survives
    /// being pruned for inactivity and re-added.
    #[serde(default)]
    pub self_member_info: Option<AuthorizedMemberInfo>,
}

impl BoardData {
    /// Regenerate the contract_key from the owner_vk using the current WASM.
    /// This ensures the contract_key always matches the bundled WASM, which may
    /// have been updated since the board was first created/stored.
    pub fn regenerate_contract_key(&mut self) {
        let params = ChatBoardParametersV1 {
            owner: self.owner_vk,
        };
        let params_bytes = to_cbor_vec(&params);
        let contract_code = ContractCode::from(BOARD_CONTRACT_WASM);
        self.contract_key =
            ContractKey::from_params_and_code(Parameters::from(params_bytes), &contract_code);
    }

    /// Get the board key for delegate operations (owner's verifying key bytes)
    pub fn board_key(&self) -> BoardKey {
        self.owner_vk.to_bytes()
    }

    /// Check if the board is in private mode
    pub fn is_private(&self) -> bool {
        matches!(
            self.board_state.configuration.configuration.privacy_mode,
            river_core::board_state::privacy::PrivacyMode::Private
        )
    }

    /// Get the current (latest) secret for encryption/decryption
    pub fn get_secret(&self) -> Option<(&[u8; 32], u32)> {
        self.current_secret_version
            .and_then(|v| self.secrets.get(&v).map(|s| (s, v)))
    }

    /// Get a secret for a specific version (for decrypting old content)
    pub fn get_secret_for_version(&self, version: u32) -> Option<&[u8; 32]> {
        self.secrets.get(&version)
    }

    /// Get a reference to the current secret (convenience method)
    pub fn current_secret(&self) -> Option<&[u8; 32]> {
        self.current_secret_version
            .and_then(|v| self.secrets.get(&v))
    }

    /// Set/add a board secret for a specific version
    pub fn set_secret(&mut self, secret: [u8; 32], version: u32) {
        self.secrets.insert(version, secret);
        // Update current version if this is a newer version
        if self.current_secret_version.is_none_or(|v| version >= v) {
            self.current_secret_version = Some(version);
            self.last_secret_rotation = Some(get_current_system_time());
        }
    }

    /// Check if the secret needs rotation (weekly rotation or never rotated)
    /// Only applies to private boards owned by this user
    pub fn needs_secret_rotation(&self) -> bool {
        // Only check for private boards
        if !self.is_private() {
            return false;
        }

        // Only the owner can rotate
        if self.owner_vk != self.self_sk.verifying_key() {
            return false;
        }

        // Check if we have a last rotation time
        match self.last_secret_rotation {
            None => {
                // Never rotated, check if board has been around for a week
                // Get the creation time from the first secret version
                if let Some(first_version) = self.board_state.secrets.versions.first() {
                    let creation_time = first_version.record.created_at;
                    if let Ok(duration) = get_current_system_time().duration_since(creation_time) {
                        // Rotate if it's been more than 7 days since creation
                        return duration.as_secs() > 7 * 24 * 60 * 60;
                    }
                }
                false
            }
            Some(last_rotation) => {
                // Check if it's been more than 7 days since last rotation
                if let Ok(duration) = get_current_system_time().duration_since(last_rotation) {
                    duration.as_secs() > 7 * 24 * 60 * 60
                } else {
                    false
                }
            }
        }
    }

    /// Check if the user can send a message in the board.
    /// A user is considered a member if they are the owner, are in the active
    /// members list, or have a stored invitation (self_authorized_member).
    pub fn can_send_message(&self) -> Result<(), SendMessageError> {
        let verifying_key = self.self_sk.verifying_key();
        let member_id = MemberId::from(&verifying_key);

        // Check if banned first
        if self
            .board_state
            .bans
            .0
            .iter()
            .any(|b| b.ban.banned_user == member_id)
        {
            return Err(SendMessageError::UserBanned);
        }

        // Owner can always send
        if verifying_key == self.owner_vk {
            return Ok(());
        }

        // Currently in members list
        if self
            .board_state
            .members
            .members
            .iter()
            .any(|m| m.member.member_vk == verifying_key)
        {
            return Ok(());
        }

        // Has stored invite (can re-add with first message)
        if self.self_authorized_member.is_some() {
            return Ok(());
        }

        Err(SendMessageError::UserNotMember)
    }

    /// Check if the user can participate in the board (send messages, edit profile).
    /// Returns Ok if user is not banned AND (is owner OR has self_authorized_member OR is in members list).
    pub fn can_participate(&self) -> Result<(), SendMessageError> {
        let verifying_key = self.self_sk.verifying_key();
        let member_id = MemberId::from(&verifying_key);

        // Check if banned first
        if self
            .board_state
            .bans
            .0
            .iter()
            .any(|b| b.ban.banned_user == member_id)
        {
            return Err(SendMessageError::UserBanned);
        }

        // Owner can always participate
        if verifying_key == self.owner_vk {
            return Ok(());
        }

        // Currently in members list
        if self
            .board_state
            .members
            .members
            .iter()
            .any(|m| m.member.member_vk == verifying_key)
        {
            return Ok(());
        }

        // Has stored invite (was previously a member, can re-add)
        if self.self_authorized_member.is_some() {
            return Ok(());
        }

        Err(SendMessageError::UserNotMember)
    }

    /// Check if the current user can create categories (must be owner or admin)
    pub fn can_create_categories(&self) -> bool {
        let self_id = MemberId::from(&self.self_sk.verifying_key());
        let owner_id = MemberId::from(&self.owner_vk);

        // Owner can always create categories
        if self_id == owner_id {
            return true;
        }

        // Check if user is an admin
        self.board_state
            .admin
            .admins
            .iter()
            .any(|a| a.admin.id() == self_id)
    }

    /// Capture the user's AuthorizedMember and MemberInfo from the current state.
    /// AuthorizedMember is only captured once (migration path for older boards).
    /// MemberInfo is always updated to the latest version so nickname edits are preserved.
    pub fn capture_self_membership_data(&mut self, parameters: &ChatBoardParametersV1) {
        let verifying_key = self.self_sk.verifying_key();
        if verifying_key == self.owner_vk {
            return; // Owner doesn't need this
        }

        // Always update self_member_info to latest version
        let member_id = MemberId::from(&verifying_key);
        if let Some(info) = self
            .board_state
            .member_info
            .member_info
            .iter()
            .filter(|i| i.member_info.member_id == member_id)
            .max_by_key(|i| i.member_info.version)
        {
            self.self_member_info = Some(info.clone());
        }

        // Only capture authorized member once
        if self.self_authorized_member.is_some() {
            return;
        }
        if let Some(member) = self
            .board_state
            .members
            .members
            .iter()
            .find(|m| m.member.member_vk == verifying_key)
        {
            self.self_authorized_member = Some(member.clone());
            // Capture invite chain
            if let Ok(chain) = self
                .board_state
                .members
                .get_invite_chain(member, parameters)
            {
                self.invite_chain = chain;
            }
        }
    }

    pub fn owner_id(&self) -> MemberId {
        self.owner_vk.into()
    }

    /// Replace an existing member entry with a new authorized member
    /// Returns true if the member was found and updated
    pub fn restore_member_access(
        &mut self,
        old_member_vk: VerifyingKey,
        new_authorized_member: AuthorizedMember,
    ) -> bool {
        // Find and replace the member entry
        if let Some(member) = self
            .board_state
            .members
            .members
            .iter_mut()
            .find(|m| m.member.member_vk == old_member_vk)
        {
            *member = new_authorized_member;
            true
        } else {
            false
        }
    }

    pub fn parameters(&self) -> ChatBoardParametersV1 {
        ChatBoardParametersV1 {
            owner: self.owner_vk,
        }
    }

    /// Rotate the board secret, generating a new secret and encrypting it for all current members
    /// This excludes banned members from receiving the new secret
    /// Returns a SecretsDelta with the new secret version and encrypted secrets
    pub fn rotate_secret(
        &mut self,
    ) -> Result<river_core::board_state::secret::SecretsDelta, String> {
        use river_core::board_state::secret::SecretsDelta;

        // Only allow rotation for private boards
        if !self.is_private() {
            return Err("Cannot rotate secret for public board".to_string());
        }

        // Only the board owner can rotate secrets
        if self.owner_vk != self.self_sk.verifying_key() {
            return Err("Only board owner can rotate secrets".to_string());
        }

        // Get current version and increment
        let new_version = self.board_state.secrets.current_version + 1;

        // Generate new secret
        let new_secret = crate::util::ecies::generate_board_secret();

        // Create the secret version record
        let secret_version = SecretVersionRecordV1 {
            version: new_version,
            cipher_spec: BoardCipherSpec::Aes256Gcm,
            created_at: get_current_system_time(),
        };

        let authorized_version = AuthorizedSecretVersionRecord::new(secret_version, &self.self_sk);

        // Get all current members, excluding banned members
        let banned_members: std::collections::HashSet<MemberId> = self
            .board_state
            .bans
            .0
            .iter()
            .map(|b| b.ban.banned_user)
            .collect();

        let current_members: Vec<MemberId> = self
            .board_state
            .members
            .members
            .iter()
            .map(|m| MemberId::from(&m.member.member_vk))
            .filter(|id| !banned_members.contains(id))
            .collect();

        if current_members.is_empty() {
            return Err("No members to encrypt secret for".to_string());
        }

        use dioxus::logger::tracing::info;
        info!(
            "Rotating secret to version {} for {} members",
            new_version,
            current_members.len()
        );

        // Encrypt the new secret for each member
        let mut new_encrypted_secrets = Vec::new();

        for member_id in current_members {
            // Find the member's verifying key
            if let Some(member) = self
                .board_state
                .members
                .members
                .iter()
                .find(|m| MemberId::from(&m.member.member_vk) == member_id)
            {
                let member_vk = member.member.member_vk;

                // Encrypt the board secret for this member
                let (ciphertext, nonce, ephemeral_key) =
                    encrypt_secret_for_member(&new_secret, &member_vk);

                // Create the encrypted secret record
                let encrypted_secret = EncryptedSecretForMemberV1 {
                    member_id,
                    secret_version: new_version,
                    ciphertext,
                    nonce,
                    sender_ephemeral_public_key: ephemeral_key.to_bytes(),
                    provider: self.owner_vk.into(),
                };

                let authorized_encrypted_secret =
                    AuthorizedEncryptedSecretForMember::new(encrypted_secret, &self.self_sk);

                new_encrypted_secrets.push(authorized_encrypted_secret);
            }
        }

        // Update our local secrets (add new version, keep old ones for decryption)
        self.secrets.insert(new_version, new_secret);
        self.current_secret_version = Some(new_version);
        self.last_secret_rotation = Some(get_current_system_time());

        Ok(SecretsDelta {
            current_version: Some(new_version),
            new_versions: vec![authorized_version],
            new_encrypted_secrets,
        })
    }

    /// Generate encrypted secrets for members who don't have them yet
    /// Returns a SecretsDelta if secrets were generated, None otherwise
    pub fn generate_missing_member_secrets(
        &self,
    ) -> Option<river_core::board_state::secret::SecretsDelta> {
        use river_core::board_state::secret::SecretsDelta;

        // Only generate secrets if this is a private board and we have the secret
        if !self.is_private() {
            return None;
        }

        let (board_secret, current_version) = self.get_secret()?;

        // Get all current members
        let member_ids: Vec<MemberId> = self
            .board_state
            .members
            .members
            .iter()
            .map(|m| MemberId::from(&m.member.member_vk))
            .collect();

        // Find members who don't have encrypted secrets for the current version
        let members_with_secrets: std::collections::HashSet<MemberId> = self
            .board_state
            .secrets
            .encrypted_secrets
            .iter()
            .filter(|s| s.secret.secret_version == current_version)
            .map(|s| s.secret.member_id)
            .collect();

        let members_without_secrets: Vec<_> = member_ids
            .into_iter()
            .filter(|id| !members_with_secrets.contains(id))
            .collect();

        if members_without_secrets.is_empty() {
            return None;
        }

        use dioxus::logger::tracing::info;
        info!(
            "Generating encrypted secrets for {} members",
            members_without_secrets.len()
        );

        // Generate encrypted secrets for each member
        let mut new_encrypted_secrets = Vec::new();

        for member_id in members_without_secrets {
            // Find the member's verifying key
            if let Some(member) = self
                .board_state
                .members
                .members
                .iter()
                .find(|m| MemberId::from(&m.member.member_vk) == member_id)
            {
                let member_vk = member.member.member_vk;

                // Encrypt the board secret for this member
                let (ciphertext, nonce, ephemeral_key) =
                    encrypt_secret_for_member(board_secret, &member_vk);

                // Create the encrypted secret record
                let encrypted_secret = EncryptedSecretForMemberV1 {
                    member_id,
                    secret_version: current_version,
                    ciphertext,
                    nonce,
                    sender_ephemeral_public_key: ephemeral_key.to_bytes(),
                    provider: self.owner_vk.into(),
                };

                let authorized_encrypted_secret =
                    AuthorizedEncryptedSecretForMember::new(encrypted_secret, &self.self_sk);

                new_encrypted_secrets.push(authorized_encrypted_secret);
            }
        }

        if new_encrypted_secrets.is_empty() {
            return None;
        }

        Some(SecretsDelta {
            current_version: None,
            new_versions: vec![],
            new_encrypted_secrets,
        })
    }
}

pub struct CurrentBoard {
    pub owner_key: Option<VerifyingKey>,
}

impl CurrentBoard {
    pub fn owner_id(&self) -> Option<MemberId> {
        self.owner_key.map(|vk| vk.into())
    }

    pub fn owner_key(&self) -> Option<&VerifyingKey> {
        self.owner_key.as_ref()
    }
}

impl PartialEq for CurrentBoard {
    fn eq(&self, other: &Self) -> bool {
        self.owner_key == other.owner_key
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Boards {
    pub map: HashMap<VerifyingKey, BoardData>,
    #[serde(default)]
    pub current_board_key: Option<VerifyingKey>,
    /// Boards whose contract key changed due to WASM update.
    /// Each entry is (owner_vk, old_contract_key) for boards where the owner
    /// should send an upgrade pointer to the old contract.
    #[serde(skip)]
    pub migrated_boards: Vec<(VerifyingKey, ContractKey)>,
}

impl PartialEq for Boards {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl Boards {
    pub fn create_new_board_with_name(
        &mut self,
        self_sk: SigningKey,
        name: String,
        nickname: String,
        is_private: bool,
    ) -> VerifyingKey {
        use dioxus::logger::tracing::info;
        info!(
            "🟢 create_new_board_with_name called: name='{}', nickname='{}', is_private={}",
            name, nickname, is_private
        );

        let owner_vk = self_sk.verifying_key();
        let mut board_state = ChatBoardStateV1::default();

        // Generate board secret if private
        info!("🟢 Creating privacy mode and secrets...");
        let (privacy_mode, board_secret, board_secret_version) = if is_private {
            info!("🟢 Generating private board secret...");
            // Generate a random 32-byte secret
            let secret = crate::util::ecies::generate_board_secret();

            // Encrypt the secret for the owner using ECIES
            let (ciphertext, nonce, ephemeral_key) = encrypt_secret_for_member(&secret, &owner_vk);

            // Create the secret version record
            let secret_version = SecretVersionRecordV1 {
                version: 0,
                cipher_spec: BoardCipherSpec::Aes256Gcm,
                created_at: get_current_system_time(),
            };

            let authorized_version = AuthorizedSecretVersionRecord::new(secret_version, &self_sk);

            // Create encrypted secret for the owner
            let encrypted_secret = EncryptedSecretForMemberV1 {
                member_id: owner_vk.into(),
                secret_version: 0,
                ciphertext,
                nonce,
                sender_ephemeral_public_key: ephemeral_key.to_bytes(),
                provider: owner_vk.into(),
            };

            let authorized_encrypted_secret =
                AuthorizedEncryptedSecretForMember::new(encrypted_secret, &self_sk);

            // Add to board state
            board_state.secrets.versions.push(authorized_version);
            board_state
                .secrets
                .encrypted_secrets
                .push(authorized_encrypted_secret);
            board_state.secrets.current_version = 0;

            info!("🟢 Private board secret generated and encrypted");
            (PrivacyMode::Private, Some(secret), Some(0u32))
        } else {
            info!("🟢 Public board, no secret needed");
            (PrivacyMode::Public, None, None)
        };

        // Set initial configuration with privacy mode
        info!("🟢 Creating configuration...");
        let config = Configuration {
            owner_member_id: owner_vk.into(),
            privacy_mode,
            display: BoardDisplayMetadata {
                name: if let Some(ref secret) = board_secret {
                    // Encrypt board name for private boards
                    use crate::util::ecies::encrypt_with_symmetric_key;
                    let (ciphertext, nonce) = encrypt_with_symmetric_key(secret, name.as_bytes());
                    SealedBytes::Private {
                        ciphertext,
                        nonce,
                        secret_version: 0,
                        declared_len_bytes: name.len() as u32,
                    }
                } else {
                    SealedBytes::public(name.into_bytes())
                },
                description: None,
            },
            ..Configuration::default()
        };
        board_state.configuration = AuthorizedConfigurationV1::new(config, &self_sk);

        // Add owner to member_info
        let owner_info = MemberInfo {
            member_id: owner_vk.into(),
            version: 0,
            preferred_nickname: if let Some(ref secret) = board_secret {
                // Encrypt nickname for private boards
                use crate::util::ecies::encrypt_with_symmetric_key;
                let (ciphertext, nonce) = encrypt_with_symmetric_key(secret, nickname.as_bytes());
                SealedBytes::Private {
                    ciphertext,
                    nonce,
                    secret_version: 0,
                    declared_len_bytes: nickname.len() as u32,
                }
            } else {
                SealedBytes::public(nickname.into_bytes())
            },
        };
        let authorized_owner_info = AuthorizedMemberInfo::new(owner_info, &self_sk);
        board_state
            .member_info
            .member_info
            .push(authorized_owner_info);

        // Generate contract key for the board
        info!("🟢 Generating contract key...");
        let parameters = ChatBoardParametersV1 { owner: owner_vk };
        let params_bytes = to_cbor_vec(&parameters);
        let contract_code = ContractCode::from(BOARD_CONTRACT_WASM);
        // Use the full ContractKey constructor that includes the code hash
        let contract_key =
            ContractKey::from_params_and_code(Parameters::from(params_bytes), &contract_code);
        info!("🟢 Contract key generated: {:?}", contract_key);

        info!("🟢 Creating BoardData struct...");
        let secrets = if let Some(secret) = board_secret {
            let mut map = HashMap::new();
            map.insert(0, secret);
            map
        } else {
            HashMap::new()
        };
        let board_data = BoardData {
            owner_vk,
            board_state,
            self_sk,
            contract_key,
            last_read_message_id: None,
            secrets,
            current_secret_version: board_secret_version,
            last_secret_rotation: if board_secret_version.is_some() {
                Some(get_current_system_time())
            } else {
                None
            },
            key_migrated_to_delegate: false, // Will be checked/migrated on startup
            self_authorized_member: None,    // Owner doesn't need this
            invite_chain: vec![],
            self_member_info: None,
        };

        info!("🟢 Inserting board into map...");
        self.map.insert(owner_vk, board_data);
        info!("🟢 create_new_board_with_name completed successfully, returning owner_vk");
        owner_vk
    }

    /// Merge the other Boards into this Boards (eg. when Boards are loaded from storage)
    pub fn merge(&mut self, other: Boards) -> Result<(), String> {
        for (vk, mut board_data) in other.map {
            // Capture the old contract key before regeneration
            let old_contract_key = board_data.contract_key;

            // Regenerate contract_key to ensure it matches the current bundled WASM
            // This handles the case where boards were stored with an older WASM version
            board_data.regenerate_contract_key();

            // If the contract key changed (WASM was updated), track for upgrade pointer
            if old_contract_key != board_data.contract_key {
                self.migrated_boards.push((vk, old_contract_key));
            }

            // If not already in the map, add the board
            if let std::collections::hash_map::Entry::Vacant(e) = self.map.entry(vk) {
                e.insert(board_data);
            } else {
                // If the board is already in the map, merge in the new data
                let self_board_data = self.map.get_mut(&vk).unwrap();
                if self_board_data.self_sk != board_data.self_sk {
                    return Err("self_sk is different".to_string());
                }
                self_board_data.board_state.merge(
                    &self_board_data.board_state.clone(),
                    &ChatBoardParametersV1 { owner: vk },
                    &board_data.board_state,
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use river_core::board_state::configuration::{AuthorizedConfigurationV1, Configuration};
    use river_core::board_state::member::{AuthorizedMember, Member};

    /// Regression test for #85: accepting an invitation for a board that already
    /// exists in the boards map must update self_sk so can_send_message() passes.
    #[test]
    fn test_can_send_message_after_self_sk_update() {
        let mut rng = rand::thread_rng();

        // Create owner
        let owner_sk = SigningKey::generate(&mut rng);
        let owner_vk = owner_sk.verifying_key();

        // Create board state with owner config
        let config = AuthorizedConfigurationV1::new(Configuration::default(), &owner_sk);
        let mut board_state = ChatBoardStateV1 {
            configuration: config,
            ..Default::default()
        };

        // Create an invitee and add them as a member
        let invitee_sk = SigningKey::generate(&mut rng);
        let invitee_vk = invitee_sk.verifying_key();
        let member = Member {
            owner_member_id: owner_vk.into(),
            invited_by: owner_vk.into(),
            member_vk: invitee_vk,
        };
        let authorized_member = AuthorizedMember::new(member, &owner_sk);
        board_state.members.members.push(authorized_member);

        // Create BoardData with a STALE self_sk (different from the invitee key)
        let stale_sk = SigningKey::generate(&mut rng);
        let params = ChatBoardParametersV1 { owner: owner_vk };
        let params_bytes = to_cbor_vec(&params);
        let contract_code = ContractCode::from(BOARD_CONTRACT_WASM);
        let contract_key =
            ContractKey::from_params_and_code(Parameters::from(params_bytes), &contract_code);

        let mut board_data = BoardData {
            owner_vk,
            board_state,
            self_sk: stale_sk,
            contract_key,
            last_read_message_id: None,
            secrets: HashMap::new(),
            current_secret_version: None,
            last_secret_rotation: None,
            key_migrated_to_delegate: false,
            self_authorized_member: None,
            invite_chain: vec![],
            self_member_info: None,
        };

        // With stale key, user should NOT be recognized as a member
        assert_eq!(
            board_data.can_send_message(),
            Err(SendMessageError::UserNotMember)
        );

        // After updating self_sk to the invitee's key, user should be a member
        board_data.self_sk = invitee_sk;
        assert_eq!(board_data.can_send_message(), Ok(()));
    }

    /// Test that capture_self_membership_data captures and updates member_info.
    #[test]
    fn test_capture_self_membership_data_preserves_nickname() {
        use river_core::board_state::privacy::SealedBytes;

        let mut rng = rand::thread_rng();
        let owner_sk = SigningKey::generate(&mut rng);
        let owner_vk = owner_sk.verifying_key();
        let invitee_sk = SigningKey::generate(&mut rng);
        let invitee_vk = invitee_sk.verifying_key();
        let member_id = MemberId::from(&invitee_vk);

        let config = AuthorizedConfigurationV1::new(Configuration::default(), &owner_sk);
        let mut board_state = ChatBoardStateV1 {
            configuration: config,
            ..Default::default()
        };

        // Add invitee as member
        let member = Member {
            owner_member_id: owner_vk.into(),
            invited_by: owner_vk.into(),
            member_vk: invitee_vk,
        };
        board_state
            .members
            .members
            .push(AuthorizedMember::new(member, &owner_sk));

        // Add member_info with a custom nickname
        let info = MemberInfo {
            member_id,
            version: 0,
            preferred_nickname: SealedBytes::public("Alice".to_string().into_bytes()),
        };
        let authorized_info = AuthorizedMemberInfo::new_with_member_key(info, &invitee_sk);
        board_state.member_info.member_info.push(authorized_info);

        let params = ChatBoardParametersV1 { owner: owner_vk };
        let params_bytes = to_cbor_vec(&params);
        let contract_code = ContractCode::from(BOARD_CONTRACT_WASM);
        let contract_key =
            ContractKey::from_params_and_code(Parameters::from(params_bytes), &contract_code);

        let mut board_data = BoardData {
            owner_vk,
            board_state,
            self_sk: invitee_sk.clone(),
            contract_key,
            last_read_message_id: None,
            secrets: HashMap::new(),
            current_secret_version: None,
            last_secret_rotation: None,
            key_migrated_to_delegate: false,
            self_authorized_member: None,
            invite_chain: vec![],
            self_member_info: None,
        };

        // Before capture, self_member_info should be None
        assert!(board_data.self_member_info.is_none());

        // Capture should populate self_member_info
        board_data.capture_self_membership_data(&params);
        assert!(board_data.self_member_info.is_some());
        let stored = board_data.self_member_info.as_ref().unwrap();
        assert_eq!(stored.member_info.member_id, member_id);
        assert_eq!(stored.member_info.version, 0);

        // Simulate nickname edit: update member_info in board_state with higher version
        let updated_info = MemberInfo {
            member_id,
            version: 1,
            preferred_nickname: SealedBytes::public("Bob".to_string().into_bytes()),
        };
        let updated_authorized =
            AuthorizedMemberInfo::new_with_member_key(updated_info, &invitee_sk);
        board_data.board_state.member_info.member_info[0] = updated_authorized;

        // Re-capture should update to latest version
        board_data.capture_self_membership_data(&params);
        let stored = board_data.self_member_info.as_ref().unwrap();
        assert_eq!(stored.member_info.version, 1);
    }
}
