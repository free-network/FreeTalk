use crate::board_state::ban::BansV1;
use crate::board_state::member::MemberId;
use crate::board_state::ChatBoardParametersV1;
use crate::util::{sign_struct, truncated_base32, verify_struct};
use crate::ChatBoardStateV1;
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use freenet_scaffold::util::{fast_hash, FastHash};
use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::{Debug, Display};
use std::hash::{Hash, Hasher};

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug, Default)]
pub struct AdminsV1 {
    pub admins: Vec<AuthorizedAdmin>,
}

impl ComposableState for AdminsV1 {
    type ParentState = ChatBoardStateV1;
    type Summary = HashSet<MemberId>;
    type Delta = AdminsDelta;
    type Parameters = ChatBoardParametersV1;

    fn verify(
        &self,
        parent_state: &Self::ParentState,
        parameters: &Self::Parameters,
    ) -> Result<(), String> {
        if self.admins.is_empty() {
            return Ok(());
        }

        if self.admins.len() > parent_state.configuration.configuration.max_admins {
            return Err(format!(
                "Too many admins: {} > {}",
                self.admins.len(),
                parent_state.configuration.configuration.max_admins
            ));
        }

        let owner_id = parameters.owner_id();

        for admin in &self.admins {
            if admin.admin.id() == owner_id {
                return Err("Owner should not be included in the admins list".to_string());
            }

            admin.verify_signature(&parameters.owner)?;
        }
        Ok(())
    }
    fn summarize(
        &self,
        _parent_state: &Self::ParentState,
        _parameters: &Self::Parameters,
    ) -> Self::Summary {
        self.admins.iter().map(|m| m.admin.id()).collect()
    }

    fn delta(
        &self,
        _parent_state: &Self::ParentState,
        _parameters: &Self::Parameters,
        old_state_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        let added = self
            .admins
            .iter()
            .filter(|m| !old_state_summary.contains(&m.admin.id()))
            .cloned()
            .collect::<Vec<_>>();
        if added.is_empty() {
            None
        } else {
            Some(AdminsDelta { added })
        }
    }

    fn apply_delta(
        &mut self,
        parent_state: &Self::ParentState,
        parameters: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        let max_admins = parent_state.configuration.configuration.max_admins;

        if let Some(delta) = delta {
            // Build a combined lookup map that includes both existing admins
            // AND admins being added in this delta. This is necessary because
            // during merge, a admin and their inviter may both be in the delta
            // (e.g., admin B invited by admin A, both being added from the
            // other state). Without this, verify would fail with "Inviter not found".
            let mut combined_admins_by_id = self.admins_by_member_id();
            for admin in &delta.added {
                combined_admins_by_id
                    .entry(admin.admin.id())
                    .or_insert(admin);
            }

            if combined_admins_by_id.len() > max_admins {
                return Err(format!(
                    "Too many admins after delta: {} > {}",
                    combined_admins_by_id.len(),
                    max_admins
                ));
            }

            // Verify that all new admins have valid signatures
            for admin in &delta.added {
                admin.verify_signature(&parameters.owner)?;
            }

            // Add ALL new admins (deduplicated), let remove_excess_admins handle trimming.
            // This ensures CRDT convergence: regardless of delta order, the same set of
            // admins will be kept based on the deterministic removal criteria.
            for admin in &delta.added {
                // Skip if this admin already exists
                if self.admins.iter().any(|m| m.admin.id() == admin.admin.id()) {
                    continue;
                }
                self.admins.push(admin.clone());
            }
        }

        // Sort for deterministic ordering (CRDT convergence requirement)
        self.admins.sort_by_key(|m| m.admin.id());

        Ok(())
    }
}

impl AdminsV1 {
    /// Note: doesn't include owner
    pub fn admins_by_member_id(&self) -> HashMap<MemberId, &AuthorizedAdmin> {
        self.admins.iter().map(|m| (m.admin.id(), m)).collect()
    }

    /* /// Checks if there are any banned admins or admins downstream of banned admins in the invite chain
    pub fn has_banned_admins(&self, bans_v1: &BansV1, parameters: &ChatBoardParametersV1) -> bool {
        self.check_banned_admins(bans_v1, parameters).is_some()
    }

    /// Removes banned admins or admins downstream of banned admins in the invite chain
    fn remove_banned_admins(&mut self, bans_v1: &BansV1, _parameters: &ChatBoardParametersV1) {
        let mut banned_ids = HashSet::new();
        for ban in &bans_v1.0 {
            banned_ids.insert(ban.ban.banned_user);
            banned_ids.extend(self.get_downstream_admins(ban.ban.banned_user));
        }
        self.admins
            .retain(|m| !banned_ids.contains(&m.admin.id()));
    }

    /// Checks for banned admins and returns a set of admin IDs to be removed if any are found.
    /// Uses chain walking without Ed25519 verification since we only need to check adminship,
    /// not cryptographic validity.
    fn check_banned_admins(
        &self,
        bans_v1: &BansV1,
        parameters: &ChatBoardParametersV1,
    ) -> Option<HashSet<MemberId>> {
        let banned_user_ids: HashSet<MemberId> =
            bans_v1.0.iter().map(|b| b.ban.banned_user).collect();
        if banned_user_ids.is_empty() {
            return None;
        }

        let admins_by_id = self.admins_by_member_id();
        let owner_id = parameters.owner_id();
        let mut result = HashSet::new();

        for m in &self.admins {
            // Walk the invite chain without Ed25519 verification
            let chain_ids = Self::invite_chain_ids(m, owner_id, &admins_by_id);
            if chain_ids.iter().any(|id| banned_user_ids.contains(id)) {
                result.insert(m.admin.id());
            }
        }

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }*/
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct AdminsDelta {
    added: Vec<AuthorizedAdmin>,
}

impl AdminsDelta {
    pub fn new(added: Vec<AuthorizedAdmin>) -> Self {
        AdminsDelta { added }
    }
}

// TODO: need to generalize to support multiple authorization mechanisms such as ghost keys

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct AuthorizedAdmin {
    pub admin: Admin,
    pub signature: Signature,
}

impl AuthorizedAdmin {
    pub fn new(admin: Admin, owner_vk: &SigningKey) -> Self {
        Self {
            admin: admin.clone(),
            signature: sign_struct(&admin, owner_vk),
        }
    }

    /// Create an AuthorizedAdmin with a pre-computed signature.
    /// Use this when signing is done externally (e.g., via delegate).
    pub fn with_signature(admin: Admin, signature: Signature) -> Self {
        Self { admin, signature }
    }

    pub fn verify_signature(&self, owner_vk: &VerifyingKey) -> Result<(), String> {
        verify_struct(&self.admin, &self.signature, owner_vk)
            .map_err(|e| format!("Invalid signature: {}", e))
    }
}

impl Hash for AuthorizedAdmin {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.admin.hash(state);
    }
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Hash, Clone)]
pub struct Admin {
    pub member_id: MemberId,
}

impl fmt::Debug for Admin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Admin")
            .field("member_id", &format_args!("{}", self.member_id))
            .finish()
    }
}

impl Admin {
    pub fn id(&self) -> MemberId {
        self.member_id.into()
    }
}
