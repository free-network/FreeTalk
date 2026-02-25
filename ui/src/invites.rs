//! Handles pending board invitations and join requests
//!
//! This module manages the state of board invitations that are in the process
//! of being accepted or retrieved.

use ed25519_dalek::{SigningKey, VerifyingKey};
use river_core::board_state::member::AuthorizedMember;
use std::collections::HashMap;

/// Collection of pending board join requests
#[derive(Clone, Debug, Default)]
pub struct PendingInvites {
    /// Map of board owner keys to pending join information
    pub map: HashMap<VerifyingKey, PendingBoardJoin>, // TODO: Make this private and use methods to access
}

impl PendingInvites {
    /// Creates a new instance of `PendingInvites`
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
    /*
        /// Adds a new pending board join request
        pub fn add(&mut self, owner_key: VerifyingKey, join_info: PendingBoardJoin) {
            self.map.insert(owner_key, join_info);
        }

        /// Removes a pending board join request
        pub fn remove(&mut self, owner_key: &VerifyingKey) {
            self.map.remove(owner_key);
        }
    */
}

/// Information about a pending board join
#[derive(Clone, Debug)]
pub struct PendingBoardJoin {
    /// The authorized member data for the join
    pub authorized_member: AuthorizedMember,
    /// The signing key for the invited member
    pub invitee_signing_key: SigningKey,
    /// User's preferred nickname for this board
    pub preferred_nickname: String,
    /// Current status of the join request
    pub status: PendingBoardStatus,
    /// Timestamp (ms since epoch) when the status moved to `Subscribing`,
    /// used to detect stuck invitations that need retry.
    pub subscribing_since: Option<f64>,
}

/// Status of a pending board join request
#[derive(Clone, Debug, PartialEq)]
pub enum PendingBoardStatus {
    /// Ready to subscribe to board data
    PendingSubscription,
    /// Subscription request sent, waiting for response
    Subscribing,
    /// Successfully subscribed and retrieved board data
    Subscribed,
    /// Error occurred during subscription or retrieval
    Error(String),
}
