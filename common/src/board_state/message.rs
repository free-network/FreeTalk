use crate::board_state::member::MemberId;
use crate::board_state::privacy::{PrivacyMode, SecretVersion};
use crate::board_state::ChatBoardParametersV1;
use crate::util::sign_struct;
use crate::util::{truncated_base64, verify_struct};
use crate::ChatBoardStateV1;
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use freenet_scaffold::util::{fast_hash, FastHash};
use freenet_scaffold::ComposableState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::time::SystemTime;

use crate::board_state::content::CategoryEditPayload;

/// Computed state for message actions (edits, deletes, reactions)
/// This is rebuilt from action messages and not serialized
#[derive(Clone, PartialEq, Debug, Default)]
pub struct MessageActionsState {
    /// Messages that have been edited: message_id -> new text content
    pub edited_content: HashMap<MessageId, String>,
    /// Categories that have been edited: message_id -> edit payload
    pub edited_categories: HashMap<MessageId, CategoryEditPayload>,
    /// Messages that have been deleted
    pub deleted: std::collections::HashSet<MessageId>,
    /// Reactions on messages: message_id -> (emoji -> list of reactors)
    pub reactions: HashMap<MessageId, HashMap<String, Vec<MemberId>>>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct MessagesV1 {
    pub messages: Vec<AuthorizedMessageV1>,
    /// Computed state from action messages (not serialized - rebuilt on each delta)
    #[serde(skip)]
    pub actions_state: MessageActionsState,
}

impl ComposableState for MessagesV1 {
    type ParentState = ChatBoardStateV1;
    type Summary = Vec<MessageId>;
    type Delta = Vec<AuthorizedMessageV1>;
    type Parameters = ChatBoardParametersV1;

    fn verify(
        &self,
        parent_state: &Self::ParentState,
        parameters: &Self::Parameters,
    ) -> Result<(), String> {
        let members_by_id = parent_state.members.members_by_member_id();
        let owner_id = parameters.owner_id();

        for message in &self.messages {
            let verifying_key = if message.message.author == owner_id {
                // Owner's messages are validated against the owner's key
                &parameters.owner
            } else if let Some(member) = members_by_id.get(&message.message.author) {
                // Regular member messages are validated against their member key
                &member.member.member_vk
            } else {
                return Err(format!(
                    "Message author not found: {:?}",
                    message.message.author
                ));
            };

            if message.validate(verifying_key).is_err() {
                return Err(format!("Invalid message signature: id:{:?}", message.id()));
            }
        }

        Ok(())
    }

    fn summarize(
        &self,
        _parent_state: &Self::ParentState,
        _parameters: &Self::Parameters,
    ) -> Self::Summary {
        self.messages.iter().map(|m| m.id()).collect()
    }

    fn delta(
        &self,
        _parent_state: &Self::ParentState,
        _parameters: &Self::Parameters,
        old_state_summary: &Self::Summary,
    ) -> Option<Self::Delta> {
        let delta: Vec<AuthorizedMessageV1> = self
            .messages
            .iter()
            .filter(|m| !old_state_summary.contains(&m.id()))
            .cloned()
            .collect();
        if delta.is_empty() {
            None
        } else {
            Some(delta)
        }
    }

    fn apply_delta(
        &mut self,
        parent_state: &Self::ParentState,
        parameters: &Self::Parameters,
        delta: &Option<Self::Delta>,
    ) -> Result<(), String> {
        let max_message_size = parent_state.configuration.configuration.max_message_size;
        let max_title_size = parent_state.configuration.configuration.max_title_size;
        let privacy_mode = &parent_state.configuration.configuration.privacy_mode;
        let current_secret_version = parent_state.secrets.current_version;

        // Validate message constraints before adding
        if let Some(delta) = delta {
            for msg in delta {
                let content = &msg.message.content;

                // Validate title constraints (for public text/reply messages)
                if let Some(title) = content.title() {
                    if title.len() > max_title_size {
                        return Err(format!(
                            "Title length {} exceeds maximum {}",
                            title.len(),
                            max_title_size
                        ));
                    }
                    if title.contains('\n') || title.contains('\r') {
                        return Err("Title cannot contain newlines".to_string());
                    }
                }

                match content {
                    BoardMessageBody::Private { secret_version, .. } => {
                        // In private mode, verify secret version matches current
                        if *privacy_mode == PrivacyMode::Private
                            && *secret_version != current_secret_version
                        {
                            return Err(format!(
                                "Private message secret version {} does not match current version {}",
                                secret_version, current_secret_version
                            ));
                        }

                        // Verify all current members have encrypted blobs for this version
                        let members = parent_state.members.members_by_member_id();
                        if !parent_state.secrets.has_complete_distribution(&members) {
                            return Err(
                                "Cannot accept private messages: incomplete secret distribution"
                                    .to_string(),
                            );
                        }
                    }
                    BoardMessageBody::Public { .. } => {
                        // In private mode, reject ALL public messages including actions
                        // Privacy is a layer - everything in a private board must be encrypted
                        if *privacy_mode == PrivacyMode::Private {
                            return Err("Cannot send public messages in private board".to_string());
                        }
                    }
                }
            }

            // Deduplicate by message ID to prevent duplicate messages from race conditions
            let existing_ids: std::collections::HashSet<_> =
                self.messages.iter().map(|m| m.id()).collect();
            self.messages.extend(
                delta
                    .iter()
                    .filter(|msg| !existing_ids.contains(&msg.id()))
                    .cloned(),
            );
        }

        // Always enforce message constraints
        // Ensure there are no messages over the size limit
        self.messages
            .retain(|m| m.message.content.content_len() <= max_message_size);

        // Ensure all messages are signed by a valid member or the board owner, remove if not
        let members_by_id = parent_state.members.members_by_member_id();
        let owner_id = MemberId::from(&parameters.owner);
        self.messages.retain(|m| {
            members_by_id.contains_key(&m.message.author) || m.message.author == owner_id
        });

        // Sort messages by time, with MessageId as secondary sort for deterministic ordering
        // (CRDT convergence requirement - without this, ties produce non-deterministic order)
        self.messages.sort_by(|a, b| {
            a.message
                .time
                .cmp(&b.message.time)
                .then_with(|| a.id().cmp(&b.id()))
        });

        // Rebuild computed state from action messages
        self.rebuild_actions_state();

        Ok(())
    }
}

impl MessagesV1 {
    /// Rebuild the computed actions state by scanning all action messages.
    ///
    /// This method only processes PUBLIC action messages. For private boards,
    /// use `rebuild_actions_state_with_decrypted` and provide the decrypted
    /// content for each private action message.
    pub fn rebuild_actions_state(&mut self) {
        self.rebuild_actions_state_with_permissions(&HashMap::new(), None, None);
    }

    /// Rebuild actions state with decrypted content (for private boards).
    /// Use `rebuild_actions_state_with_permissions` if you need to allow
    /// owner/admin category edits.
    pub fn rebuild_actions_state_with_decrypted(
        &mut self,
        decrypted_content: &HashMap<MessageId, Vec<u8>>,
    ) {
        self.rebuild_actions_state_with_permissions(decrypted_content, None, None);
    }

    /// Rebuild actions state with owner/admin permissions for category edits.
    pub fn rebuild_actions_state_with_permissions(
        &mut self,
        decrypted_content: &HashMap<MessageId, Vec<u8>>,
        owner_id: Option<MemberId>,
        admin_ids: Option<&std::collections::HashSet<MemberId>>,
    ) {
        use crate::board_state::content::{
            ActionContentV1, DecodedContent, ACTION_TYPE_DELETE, ACTION_TYPE_EDIT,
            ACTION_TYPE_EDIT_CATEGORY, ACTION_TYPE_REACTION, ACTION_TYPE_REMOVE_REACTION,
        };

        // Clear existing computed state
        self.actions_state = MessageActionsState::default();

        // Build a map of message_id -> author for authorization checks
        let message_authors: HashMap<MessageId, MemberId> = self
            .messages
            .iter()
            .filter(|m| !m.message.content.is_action())
            .map(|m| (m.id(), m.message.author))
            .collect();

        // Build a set of category message IDs for type checking
        let category_ids: std::collections::HashSet<MessageId> = self
            .messages
            .iter()
            .filter(|m| m.message.content.is_category())
            .map(|m| m.id())
            .collect();

        // Process action messages in timestamp order (messages are already sorted)
        for msg in &self.messages {
            let actor = msg.message.author;

            // Skip non-action messages
            if !msg.message.content.is_action() {
                continue;
            }

            // Decode the action content - either from public data or decrypted bytes
            let action = match &msg.message.content {
                BoardMessageBody::Public { .. } => {
                    // Public action - decode directly
                    match msg.message.content.decode_content() {
                        Some(DecodedContent::Action(action)) => action,
                        _ => continue,
                    }
                }
                BoardMessageBody::Private { .. } => {
                    // Private action - use provided decrypted content
                    let msg_id = msg.id();
                    if let Some(plaintext) = decrypted_content.get(&msg_id) {
                        match ActionContentV1::decode(plaintext) {
                            Ok(action) => action,
                            Err(_) => continue,
                        }
                    } else {
                        // No decrypted content provided - skip this action
                        continue;
                    }
                }
            };

            let target = &action.target;

            match action.action_type {
                ACTION_TYPE_EDIT => {
                    // Only the original author can edit their message
                    // Categories must use ACTION_TYPE_EDIT_CATEGORY instead
                    if category_ids.contains(target) {
                        continue;
                    }
                    if let Some(&original_author) = message_authors.get(target) {
                        if actor == original_author {
                            // Don't allow editing deleted messages
                            if !self.actions_state.deleted.contains(target) {
                                if let Some(payload) = action.edit_payload() {
                                    self.actions_state
                                        .edited_content
                                        .insert(target.clone(), payload.new_text);
                                }
                            }
                        }
                    }
                }
                ACTION_TYPE_DELETE => {
                    // Only the original author can delete their message
                    if let Some(&original_author) = message_authors.get(target) {
                        if actor == original_author {
                            self.actions_state.deleted.insert(target.clone());
                            // Also remove any edited content for deleted messages
                            self.actions_state.edited_content.remove(target);
                            self.actions_state.edited_categories.remove(target);
                        }
                    }
                }
                ACTION_TYPE_REACTION => {
                    // Anyone can add reactions to non-deleted messages
                    if message_authors.contains_key(target)
                        && !self.actions_state.deleted.contains(target)
                    {
                        if let Some(payload) = action.reaction_payload() {
                            let reactions = self
                                .actions_state
                                .reactions
                                .entry(target.clone())
                                .or_default();
                            let reactors = reactions.entry(payload.emoji).or_default();
                            // Idempotent: only add if not already present
                            if !reactors.contains(&actor) {
                                reactors.push(actor);
                            }
                        }
                    }
                }
                ACTION_TYPE_REMOVE_REACTION => {
                    // Users can only remove their own reactions
                    if let Some(payload) = action.reaction_payload() {
                        if let Some(reactions) = self.actions_state.reactions.get_mut(target) {
                            if let Some(reactors) = reactions.get_mut(&payload.emoji) {
                                reactors.retain(|r| r != &actor);
                                // Clean up empty entries
                                if reactors.is_empty() {
                                    reactions.remove(&payload.emoji);
                                }
                            }
                            if reactions.is_empty() {
                                self.actions_state.reactions.remove(target);
                            }
                        }
                    }
                }
                ACTION_TYPE_EDIT_CATEGORY => {
                    // Only categories can be edited with this action type
                    if !category_ids.contains(target) {
                        continue;
                    }
                    // Owner, admins, or original author can edit categories
                    let is_owner = owner_id.map_or(false, |id| actor == id);
                    let is_admin = admin_ids.map_or(false, |ids| ids.contains(&actor));
                    let is_author = message_authors.get(target) == Some(&actor);

                    if is_owner || is_admin || is_author {
                        // Don't allow editing deleted categories
                        if !self.actions_state.deleted.contains(target) {
                            if let Some(payload) = action.category_edit_payload() {
                                self.actions_state
                                    .edited_categories
                                    .insert(target.clone(), payload);
                            }
                        }
                    }
                }
                _ => {
                    // Unknown action type - ignore for forward compatibility
                }
            }
        }
    }

    /// Check if a message has been edited
    pub fn is_edited(&self, message_id: &MessageId) -> bool {
        self.actions_state.edited_content.contains_key(message_id)
    }

    /// Check if a category has been edited
    pub fn is_category_edited(&self, message_id: &MessageId) -> bool {
        self.actions_state.edited_categories.contains_key(message_id)
    }

    /// Get the edited category payload if available
    pub fn edited_category(&self, message_id: &MessageId) -> Option<&CategoryEditPayload> {
        self.actions_state.edited_categories.get(message_id)
    }

    /// Check if a message has been deleted
    pub fn is_deleted(&self, message_id: &MessageId) -> bool {
        self.actions_state.deleted.contains(message_id)
    }

    /// Get the effective text content for a message (edited content if edited, original otherwise)
    /// Returns the text content as a string, or None if the message is encrypted/undecodable
    pub fn effective_text(&self, message: &AuthorizedMessageV1) -> Option<String> {
        let id = message.id();
        // Check if there's edited content first
        if let Some(edited_text) = self.actions_state.edited_content.get(&id) {
            return Some(edited_text.clone());
        }
        // Otherwise return the original content's text
        message.message.content.as_public_string()
    }

    /// Get reactions for a message
    pub fn reactions(&self, message_id: &MessageId) -> Option<&HashMap<String, Vec<MemberId>>> {
        self.actions_state.reactions.get(message_id)
    }

    /// Get all non-deleted, non-action messages for display
    pub fn display_messages(&self) -> impl Iterator<Item = &AuthorizedMessageV1> {
        self.messages.iter().filter(|m| {
            !m.message.content.is_action() && !self.actions_state.deleted.contains(&m.id())
        })
    }
}

/// Message body that can be either public or private (encrypted).
///
/// Content is opaque to the contract - interpretation happens client-side.
/// This design enables adding new content types without contract redeployment.
///
/// # Content Types
/// - `content_type = 1`: Text message (TextContentV1)
/// - `content_type = 2`: Action on another message (ActionContentV1)
/// - Future types can be added without contract changes
///
/// # Extensibility
/// - New content types: Just use a new content_type number
/// - New action types: Just use a new action_type number within ActionContentV1
/// - New fields: Add to content structs (old clients ignore unknown fields)
/// - Breaking changes: Bump content_version
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum BoardMessageBody {
    /// Public (unencrypted) message
    Public {
        /// Content type identifier (see content module for constants)
        content_type: u32,
        /// Version of the content format
        content_version: u32,
        /// CBOR-encoded content bytes
        data: Vec<u8>,
    },
    /// Private (encrypted) message
    Private {
        /// Content type identifier (see content module for constants)
        content_type: u32,
        /// Version of the content format
        content_version: u32,
        /// Encrypted CBOR-encoded content
        ciphertext: Vec<u8>,
        /// Nonce used for encryption
        nonce: [u8; 12],
        /// Version of the board secret used for encryption
        secret_version: SecretVersion,
    },
}

impl BoardMessageBody {
    /// Create a new public text message
    pub fn public(title: String, content: String) -> Self {
        use crate::board_state::content::{TextContentV1, CONTENT_TYPE_TEXT, TEXT_CONTENT_VERSION};
        let text_content = TextContentV1::new(title, content);
        Self::Public {
            content_type: CONTENT_TYPE_TEXT,
            content_version: TEXT_CONTENT_VERSION,
            data: text_content.encode(),
        }
    }

    /// Create a new public message with raw content
    pub fn public_raw(content_type: u32, content_version: u32, data: Vec<u8>) -> Self {
        Self::Public {
            content_type,
            content_version,
            data,
        }
    }

    /// Create a new private message
    pub fn private(
        content_type: u32,
        content_version: u32,
        ciphertext: Vec<u8>,
        nonce: [u8; 12],
        secret_version: SecretVersion,
    ) -> Self {
        Self::Private {
            content_type,
            content_version,
            ciphertext,
            nonce,
            secret_version,
        }
    }

    /// Create a private text message (convenience method)
    pub fn private_text(
        ciphertext: Vec<u8>,
        nonce: [u8; 12],
        secret_version: SecretVersion,
    ) -> Self {
        use crate::board_state::content::{CONTENT_TYPE_TEXT, TEXT_CONTENT_VERSION};
        Self::Private {
            content_type: CONTENT_TYPE_TEXT,
            content_version: TEXT_CONTENT_VERSION,
            ciphertext,
            nonce,
            secret_version,
        }
    }

    /// Create an edit action (public)
    pub fn edit(target: MessageId, new_title: String, new_text: String) -> Self {
        use crate::board_state::content::{
            ActionContentV1, ACTION_CONTENT_VERSION, CONTENT_TYPE_ACTION,
        };
        let action = ActionContentV1::edit(target, new_title, new_text);
        Self::Public {
            content_type: CONTENT_TYPE_ACTION,
            content_version: ACTION_CONTENT_VERSION,
            data: action.encode(),
        }
    }

    /// Create a delete action (public)
    pub fn delete(target: MessageId) -> Self {
        use crate::board_state::content::{
            ActionContentV1, ACTION_CONTENT_VERSION, CONTENT_TYPE_ACTION,
        };
        let action = ActionContentV1::delete(target);
        Self::Public {
            content_type: CONTENT_TYPE_ACTION,
            content_version: ACTION_CONTENT_VERSION,
            data: action.encode(),
        }
    }

    /// Create a reaction action (public)
    pub fn reaction(target: MessageId, emoji: String) -> Self {
        use crate::board_state::content::{
            ActionContentV1, ACTION_CONTENT_VERSION, CONTENT_TYPE_ACTION,
        };
        let action = ActionContentV1::reaction(target, emoji);
        Self::Public {
            content_type: CONTENT_TYPE_ACTION,
            content_version: ACTION_CONTENT_VERSION,
            data: action.encode(),
        }
    }

    /// Create a remove reaction action (public)
    pub fn remove_reaction(target: MessageId, emoji: String) -> Self {
        use crate::board_state::content::{
            ActionContentV1, ACTION_CONTENT_VERSION, CONTENT_TYPE_ACTION,
        };
        let action = ActionContentV1::remove_reaction(target, emoji);
        Self::Public {
            content_type: CONTENT_TYPE_ACTION,
            content_version: ACTION_CONTENT_VERSION,
            data: action.encode(),
        }
    }

    /// Create an edit category action (public)
    pub fn edit_category(
        target: MessageId,
        new_name: String,
        new_description: Option<String>,
        new_icon: Option<String>,
        new_color: String,
    ) -> Self {
        use crate::board_state::content::{
            ActionContentV1, ACTION_CONTENT_VERSION, CONTENT_TYPE_ACTION,
        };
        let action = ActionContentV1::edit_category(target, new_name, new_description, new_icon, new_color);
        Self::Public {
            content_type: CONTENT_TYPE_ACTION,
            content_version: ACTION_CONTENT_VERSION,
            data: action.encode(),
        }
    }

    /// Create a public reply message
    pub fn reply(
        title: String,
        content: String,
        target_message_id: MessageId,
        target_author_name: String,
        target_content_preview: String,
    ) -> Self {
        use crate::board_state::content::{
            ReplyContentV1, CONTENT_TYPE_REPLY, REPLY_CONTENT_VERSION,
        };
        let reply = ReplyContentV1::new(
            title,
            content,
            target_message_id,
            target_author_name,
            target_content_preview,
        );
        Self::Public {
            content_type: CONTENT_TYPE_REPLY,
            content_version: REPLY_CONTENT_VERSION,
            data: reply.encode(),
        }
    }

    /// Create a public category message (for organizing posts)
    ///
    /// Only board owners and admins should create categories.
    /// The caller must verify permissions before calling this.
    pub fn category(
        name: String,
        description: Option<String>,
        icon: Option<String>,
        color: String,
        parent_category_id: Option<MessageId>,
    ) -> Self {
        use crate::board_state::content::{
            CategoryContentV1, CATEGORY_CONTENT_VERSION, CONTENT_TYPE_CATEGORY,
        };
        let mut cat = CategoryContentV1::new(name, description, color);
        if let Some(i) = icon {
            cat = cat.with_icon(i);
        }
        if let Some(p) = parent_category_id {
            cat = cat.with_parent(p);
        }
        Self::Public {
            content_type: CONTENT_TYPE_CATEGORY,
            content_version: CATEGORY_CONTENT_VERSION,
            data: cat.encode(),
        }
    }

    /// Create a private action message (encrypted)
    ///
    /// Use this for any action (edit, delete, reaction, remove_reaction) in a private board.
    /// The caller should:
    /// 1. Create the ActionContentV1 (e.g., `ActionContentV1::edit(target, new_text)`)
    /// 2. Encode it: `action.encode()`
    /// 3. Encrypt the bytes with the board secret
    /// 4. Pass the ciphertext here
    pub fn private_action(
        ciphertext: Vec<u8>,
        nonce: [u8; 12],
        secret_version: SecretVersion,
    ) -> Self {
        use crate::board_state::content::{ACTION_CONTENT_VERSION, CONTENT_TYPE_ACTION};
        Self::Private {
            content_type: CONTENT_TYPE_ACTION,
            content_version: ACTION_CONTENT_VERSION,
            ciphertext,
            nonce,
            secret_version,
        }
    }

    /// Check if this is a public message
    pub fn is_public(&self) -> bool {
        matches!(self, Self::Public { .. })
    }

    /// Check if this is a private message
    pub fn is_private(&self) -> bool {
        matches!(self, Self::Private { .. })
    }

    /// Get the content type
    pub fn content_type(&self) -> u32 {
        match self {
            Self::Public { content_type, .. } | Self::Private { content_type, .. } => *content_type,
        }
    }

    /// Get the content version
    pub fn content_version(&self) -> u32 {
        match self {
            Self::Public {
                content_version, ..
            }
            | Self::Private {
                content_version, ..
            } => *content_version,
        }
    }

    /// Check if this is an action message (content_type = ACTION)
    pub fn is_action(&self) -> bool {
        use crate::board_state::content::CONTENT_TYPE_ACTION;
        self.content_type() == CONTENT_TYPE_ACTION
    }

    /// Check if this is a category message (content_type = CATEGORY)
    pub fn is_category(&self) -> bool {
        use crate::board_state::content::CONTENT_TYPE_CATEGORY;
        self.content_type() == CONTENT_TYPE_CATEGORY
    }

    /// Decode the content (for public messages only)
    /// Returns None for private messages - decrypt first
    pub fn decode_content(&self) -> Option<crate::board_state::content::DecodedContent> {
        use crate::board_state::content::{
            ActionContentV1, CategoryContentV1, DecodedContent, ReplyContentV1, TextContentV1,
            CONTENT_TYPE_ACTION, CONTENT_TYPE_CATEGORY, CONTENT_TYPE_REPLY, CONTENT_TYPE_TEXT,
        };
        match self {
            Self::Public {
                content_type,
                content_version,
                data,
            } => match *content_type {
                CONTENT_TYPE_TEXT => TextContentV1::decode(data).ok().map(DecodedContent::Text),
                CONTENT_TYPE_ACTION => ActionContentV1::decode(data)
                    .ok()
                    .map(DecodedContent::Action),
                CONTENT_TYPE_REPLY => ReplyContentV1::decode(data).ok().map(DecodedContent::Reply),
                CONTENT_TYPE_CATEGORY => {
                    CategoryContentV1::decode(data).ok().map(DecodedContent::Category)
                }
                _ => Some(DecodedContent::Unknown {
                    content_type: *content_type,
                    content_version: *content_version,
                }),
            },
            Self::Private { .. } => None,
        }
    }

    /// Get the target message ID if this is an action
    pub fn target_id(&self) -> Option<MessageId> {
        use crate::board_state::content::{ActionContentV1, CONTENT_TYPE_ACTION};
        match self {
            Self::Public {
                content_type, data, ..
            } if *content_type == CONTENT_TYPE_ACTION => {
                ActionContentV1::decode(data).ok().map(|a| a.target)
            }
            _ => None,
        }
    }

    /// Get the content length for validation (contract uses this for size limits)
    pub fn content_len(&self) -> usize {
        match self {
            Self::Public { data, .. } => data.len(),
            Self::Private { ciphertext, .. } => ciphertext.len(),
        }
    }

    /// Get the secret version (if private)
    pub fn secret_version(&self) -> Option<SecretVersion> {
        match self {
            Self::Public { .. } => None,
            Self::Private { secret_version, .. } => Some(*secret_version),
        }
    }

    /// Get a string representation for display purposes
    pub fn to_string_lossy(&self) -> String {
        match self {
            Self::Public { .. } => {
                if let Some(decoded) = self.decode_content() {
                    decoded.to_display_string()
                } else {
                    "[Failed to decode message]".to_string()
                }
            }
            Self::Private {
                ciphertext,
                secret_version,
                ..
            } => {
                format!(
                    "[Encrypted message: {} bytes, v{}]",
                    ciphertext.len(),
                    secret_version
                )
            }
        }
    }

    /// Try to get the public plaintext, returns None if private or not a text message
    pub fn as_public_string(&self) -> Option<String> {
        self.decode_content()
            .and_then(|c| c.as_text().map(|s| s.to_string()))
    }

    /// Extract title from the message content (for public messages only).
    /// Returns None for private messages or non-text/reply/category content.
    pub fn title(&self) -> Option<String> {
        use crate::board_state::content::{
            CategoryContentV1, ReplyContentV1, TextContentV1, CONTENT_TYPE_CATEGORY,
            CONTENT_TYPE_REPLY, CONTENT_TYPE_TEXT,
        };
        match self {
            Self::Public {
                content_type, data, ..
            } => match *content_type {
                CONTENT_TYPE_TEXT => TextContentV1::decode(data).ok().map(|t| t.title),
                CONTENT_TYPE_REPLY => ReplyContentV1::decode(data).ok().map(|r| r.title),
                CONTENT_TYPE_CATEGORY => CategoryContentV1::decode(data).ok().map(|c| c.name),
                _ => None,
            },
            Self::Private { .. } => None,
        }
    }
}

impl fmt::Display for BoardMessageBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string_lossy())
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct MessageV1 {
    pub board_owner: MemberId,
    pub author: MemberId,
    pub time: SystemTime,
    pub content: BoardMessageBody,
}

impl Default for MessageV1 {
    fn default() -> Self {
        Self {
            board_owner: MemberId(FastHash(0)),
            author: MemberId(FastHash(0)),
            time: SystemTime::UNIX_EPOCH,
            content: BoardMessageBody::public(String::new(), String::new()),
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorizedMessageV1 {
    pub message: MessageV1,
    pub signature: Signature,
}

impl fmt::Debug for AuthorizedMessageV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthorizedMessage")
            .field("message", &self.message)
            .field(
                "signature",
                &format_args!("{}", truncated_base64(self.signature.to_bytes())),
            )
            .finish()
    }
}

#[derive(Eq, PartialEq, Hash, Serialize, Deserialize, Clone, Debug, Ord, PartialOrd)]
pub struct MessageId(pub FastHash);

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl AuthorizedMessageV1 {
    pub fn new(message: MessageV1, signing_key: &SigningKey) -> Self {
        Self {
            message: message.clone(),
            signature: sign_struct(&message, signing_key),
        }
    }

    /// Create an AuthorizedMessageV1 with a pre-computed signature.
    /// Use this when signing is done externally (e.g., via delegate).
    pub fn with_signature(message: MessageV1, signature: Signature) -> Self {
        Self { message, signature }
    }

    pub fn validate(
        &self,
        verifying_key: &VerifyingKey,
    ) -> Result<(), ed25519_dalek::SignatureError> {
        verify_struct(&self.message, &self.signature, verifying_key)
    }

    pub fn id(&self) -> MessageId {
        MessageId(fast_hash(&self.signature.to_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;
    use std::time::Duration;

    fn create_test_message(owner_id: MemberId, author_id: MemberId) -> MessageV1 {
        MessageV1 {
            board_owner: owner_id,
            author: author_id,
            time: SystemTime::now(),
            content: BoardMessageBody::public(String::new(), "Test message".to_string()),
        }
    }

    #[test]
    fn test_messages_v1_default() {
        let default_messages = MessagesV1::default();
        assert!(default_messages.messages.is_empty());
    }

    #[test]
    fn test_authorized_message_v1_debug() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let owner_id = MemberId(FastHash(0));
        let author_id = MemberId(FastHash(1));

        let message = create_test_message(owner_id, author_id);
        let authorized_message = AuthorizedMessageV1::new(message, &signing_key);

        let debug_output = format!("{:?}", authorized_message);
        assert!(debug_output.contains("AuthorizedMessage"));
        assert!(debug_output.contains("message"));
        assert!(debug_output.contains("signature"));
    }

    #[test]
    fn test_authorized_message_new_and_validate() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let owner_id = MemberId(FastHash(0));
        let author_id = MemberId(FastHash(1));

        let message = create_test_message(owner_id, author_id);
        let authorized_message = AuthorizedMessageV1::new(message.clone(), &signing_key);

        assert_eq!(authorized_message.message, message);
        assert!(authorized_message.validate(&verifying_key).is_ok());

        // Test with wrong key
        let wrong_key = SigningKey::generate(&mut OsRng).verifying_key();
        assert!(authorized_message.validate(&wrong_key).is_err());

        // Test with tampered message
        let mut tampered_message = authorized_message.clone();
        tampered_message.message.content =
            BoardMessageBody::public(String::new(), "Tampered content".to_string());
        assert!(tampered_message.validate(&verifying_key).is_err());
    }

    #[test]
    fn test_message_id() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let owner_id = MemberId(FastHash(0));
        let author_id = MemberId(FastHash(1));

        let message = create_test_message(owner_id, author_id);
        let authorized_message = AuthorizedMessageV1::new(message, &signing_key);

        let id1 = authorized_message.id();
        let id2 = authorized_message.id();

        assert_eq!(id1, id2);

        // Test that different messages have different IDs
        let message2 = create_test_message(owner_id, author_id);
        let authorized_message2 = AuthorizedMessageV1::new(message2, &signing_key);
        assert_ne!(authorized_message.id(), authorized_message2.id());
    }

    #[test]
    fn test_messages_verify() {
        // Generate a new signing key and its corresponding verifying key for the owner
        let owner_signing_key = SigningKey::generate(&mut OsRng);
        let owner_verifying_key = owner_signing_key.verifying_key();
        let owner_id = MemberId::from(&owner_verifying_key);

        // Generate a signing key for the author
        let author_signing_key = SigningKey::generate(&mut OsRng);
        let author_verifying_key = author_signing_key.verifying_key();
        let author_id = MemberId::from(&author_verifying_key);

        // Create a test message and authorize it with the author's signing key
        let message = create_test_message(owner_id, author_id);
        let authorized_message = AuthorizedMessageV1::new(message, &author_signing_key);

        // Create a Messages struct with the authorized message
        let messages = MessagesV1 {
            messages: vec![authorized_message],
            ..Default::default()
        };

        // Set up a parent board_state (ChatBoardState) with the author as a member
        let mut parent_state = ChatBoardStateV1::default();
        let author_member = crate::board_state::member::Member {
            owner_member_id: owner_id,
            invited_by: owner_id,
            member_vk: author_verifying_key,
        };
        let authorized_author =
            crate::board_state::member::AuthorizedMember::new(author_member, &owner_signing_key);
        parent_state.members.members = vec![authorized_author];

        // Set up parameters for verification
        let parameters = ChatBoardParametersV1 {
            owner: owner_verifying_key,
        };

        // Verify that a valid message passes verification
        assert!(
            messages.verify(&parent_state, &parameters).is_ok(),
            "Valid messages should pass verification: {:?}",
            messages.verify(&parent_state, &parameters)
        );

        // Test with invalid signature
        let mut invalid_messages = messages.clone();
        invalid_messages.messages[0].signature = Signature::from_bytes(&[0; 64]); // Replace with an invalid signature
        assert!(
            invalid_messages.verify(&parent_state, &parameters).is_err(),
            "Messages with invalid signature should fail verification"
        );

        // Test with non-existent author
        let non_existent_author_id =
            MemberId::from(&SigningKey::generate(&mut OsRng).verifying_key());
        let invalid_message = create_test_message(owner_id, non_existent_author_id);
        let invalid_authorized_message =
            AuthorizedMessageV1::new(invalid_message, &author_signing_key);
        let invalid_messages = MessagesV1 {
            messages: vec![invalid_authorized_message],
            ..Default::default()
        };
        assert!(
            invalid_messages.verify(&parent_state, &parameters).is_err(),
            "Messages with non-existent author should fail verification"
        );
    }

    #[test]
    fn test_messages_summarize() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let owner_id = MemberId(FastHash(0));
        let author_id = MemberId(FastHash(1));

        let message1 = create_test_message(owner_id, author_id);
        let message2 = create_test_message(owner_id, author_id);

        let authorized_message1 = AuthorizedMessageV1::new(message1, &signing_key);
        let authorized_message2 = AuthorizedMessageV1::new(message2, &signing_key);

        let messages = MessagesV1 {
            messages: vec![authorized_message1.clone(), authorized_message2.clone()],
            ..Default::default()
        };

        let parent_state = ChatBoardStateV1::default();
        let parameters = ChatBoardParametersV1 {
            owner: signing_key.verifying_key(),
        };

        let summary = messages.summarize(&parent_state, &parameters);
        assert_eq!(summary.len(), 2);
        assert_eq!(summary[0], authorized_message1.id());
        assert_eq!(summary[1], authorized_message2.id());

        // Test empty messages
        let empty_messages = MessagesV1::default();
        let empty_summary = empty_messages.summarize(&parent_state, &parameters);
        assert!(empty_summary.is_empty());
    }

    #[test]
    fn test_messages_delta() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let owner_id = MemberId(FastHash(0));
        let author_id = MemberId(FastHash(1));

        let message1 = create_test_message(owner_id, author_id);
        let message2 = create_test_message(owner_id, author_id);
        let message3 = create_test_message(owner_id, author_id);

        let authorized_message1 = AuthorizedMessageV1::new(message1, &signing_key);
        let authorized_message2 = AuthorizedMessageV1::new(message2, &signing_key);
        let authorized_message3 = AuthorizedMessageV1::new(message3, &signing_key);

        let messages = MessagesV1 {
            messages: vec![
                authorized_message1.clone(),
                authorized_message2.clone(),
                authorized_message3.clone(),
            ],
            ..Default::default()
        };

        let parent_state = ChatBoardStateV1::default();
        let parameters = ChatBoardParametersV1 {
            owner: signing_key.verifying_key(),
        };

        // Test with partial old summary
        let old_summary = vec![authorized_message1.id(), authorized_message2.id()];
        let delta = messages
            .delta(&parent_state, &parameters, &old_summary)
            .unwrap();
        assert_eq!(delta.len(), 1);
        assert_eq!(delta[0], authorized_message3);

        // Test with empty old summary
        let empty_summary: Vec<MessageId> = vec![];
        let full_delta = messages
            .delta(&parent_state, &parameters, &empty_summary)
            .unwrap();
        assert_eq!(full_delta.len(), 3);
        assert_eq!(full_delta, messages.messages);

        // Test with full old summary (no changes)
        let full_summary = vec![
            authorized_message1.id(),
            authorized_message2.id(),
            authorized_message3.id(),
        ];
        let no_delta = messages.delta(&parent_state, &parameters, &full_summary);
        assert!(no_delta.is_none());
    }

    #[test]
    fn test_messages_apply_delta() {
        // Setup
        let owner_signing_key = SigningKey::generate(&mut OsRng);
        let owner_verifying_key = owner_signing_key.verifying_key();
        let owner_id = MemberId::from(&owner_verifying_key);

        let author_signing_key = SigningKey::generate(&mut OsRng);
        let author_verifying_key = author_signing_key.verifying_key();
        let author_id = MemberId::from(&author_verifying_key);

        let mut parent_state = ChatBoardStateV1::default();
        parent_state.configuration.configuration.max_message_size = 100;
        parent_state.members.members = vec![crate::board_state::member::AuthorizedMember {
            member: crate::board_state::member::Member {
                owner_member_id: owner_id,
                invited_by: owner_id,
                member_vk: author_verifying_key,
            },
            signature: owner_signing_key.try_sign(&[0; 32]).unwrap(),
        }];

        let parameters = ChatBoardParametersV1 {
            owner: owner_verifying_key,
        };

        // Create messages
        let create_message = |time: SystemTime| {
            let message = MessageV1 {
                board_owner: owner_id,
                author: author_id,
                time,
                content: BoardMessageBody::public(String::new(), "Test message".to_string()),
            };
            AuthorizedMessageV1::new(message, &author_signing_key)
        };

        let now = SystemTime::now();
        let message1 = create_message(now - Duration::from_secs(3));
        let message2 = create_message(now - Duration::from_secs(2));
        let message3 = create_message(now - Duration::from_secs(1));
        let message4 = create_message(now);

        // Initial board_state with 2 messages
        let mut messages = MessagesV1 {
            messages: vec![message1.clone(), message2.clone()],
            ..Default::default()
        };

        // Apply delta with 2 new messages
        let delta = vec![message3.clone(), message4.clone()];
        assert!(messages
            .apply_delta(&parent_state, &parameters, &Some(delta))
            .is_ok());

        // Check results - all messages are kept (no pruning)
        assert_eq!(
            messages.messages.len(),
            4,
            "Should have 4 messages after applying delta"
        );
        assert!(
            messages.messages.contains(&message1),
            "Message1 should be retained"
        );
        assert!(
            messages.messages.contains(&message2),
            "Message2 should be retained"
        );
        assert!(
            messages.messages.contains(&message3),
            "New message should be added"
        );
        assert!(
            messages.messages.contains(&message4),
            "Newest message should be added"
        );

        // Apply delta with an older message
        let old_message = create_message(now - Duration::from_secs(4));
        let delta = vec![old_message.clone()];
        assert!(messages
            .apply_delta(&parent_state, &parameters, &Some(delta))
            .is_ok());

        // Check results - all messages are kept (no pruning)
        assert_eq!(messages.messages.len(), 5, "Should have 5 messages");
        assert!(
            messages.messages.contains(&old_message),
            "Older message should be added"
        );
        assert!(
            messages.messages.contains(&message1),
            "Message1 should be retained"
        );
        assert!(
            messages.messages.contains(&message2),
            "Message2 should be retained"
        );
        assert!(
            messages.messages.contains(&message3),
            "Message3 should be retained"
        );
        assert!(
            messages.messages.contains(&message4),
            "Newest message should be retained"
        );
    }

    #[test]
    fn test_oversized_message_filtered_by_apply_delta() {
        let owner_sk = SigningKey::generate(&mut OsRng);
        let owner_vk = owner_sk.verifying_key();
        let owner_id = MemberId::from(&owner_vk);

        let author_sk = SigningKey::generate(&mut OsRng);
        let author_vk = author_sk.verifying_key();
        let author_id = MemberId::from(&author_vk);

        let mut parent_state = ChatBoardStateV1::default();
        parent_state.configuration.configuration.max_message_size = 50;
        parent_state.members.members = vec![crate::board_state::member::AuthorizedMember {
            member: crate::board_state::member::Member {
                owner_member_id: owner_id,
                invited_by: owner_id,
                member_vk: author_vk,
            },
            signature: owner_sk.try_sign(&[0; 32]).unwrap(),
        }];

        let parameters = ChatBoardParametersV1 { owner: owner_vk };

        // Create a normal-sized message and an oversized message
        let small_msg = AuthorizedMessageV1::new(
            MessageV1 {
                board_owner: owner_id,
                author: author_id,
                time: SystemTime::now(),
                content: BoardMessageBody::public(String::new(), "short".to_string()),
            },
            &author_sk,
        );
        let big_msg = AuthorizedMessageV1::new(
            MessageV1 {
                board_owner: owner_id,
                author: author_id,
                time: SystemTime::now(),
                content: BoardMessageBody::public(String::new(), "x".repeat(100)),
            },
            &author_sk,
        );

        assert!(small_msg.message.content.content_len() <= 50);
        assert!(big_msg.message.content.content_len() > 50);

        let mut messages = MessagesV1::default();
        let delta = vec![small_msg.clone(), big_msg.clone()];
        assert!(messages
            .apply_delta(&parent_state, &parameters, &Some(delta))
            .is_ok());

        assert_eq!(
            messages.messages.len(),
            1,
            "Only small message should survive"
        );
        assert!(messages.messages.contains(&small_msg));
        assert!(
            !messages.messages.contains(&big_msg),
            "Oversized message should be filtered"
        );
    }

    #[test]
    fn test_message_author_preservation_across_users() {
        // Create two users
        let user1_sk = SigningKey::generate(&mut OsRng);
        let user1_vk = user1_sk.verifying_key();
        let user1_id = MemberId::from(&user1_vk);

        let user2_sk = SigningKey::generate(&mut OsRng);
        let user2_vk = user2_sk.verifying_key();
        let user2_id = MemberId::from(&user2_vk);

        let owner_sk = SigningKey::generate(&mut OsRng);
        let owner_vk = owner_sk.verifying_key();
        let owner_id = MemberId::from(&owner_vk);

        println!("User1 ID: {}", user1_id);
        println!("User2 ID: {}", user2_id);
        println!("Owner ID: {}", owner_id);

        // Create messages from different users
        let msg1 = MessageV1 {
            board_owner: owner_id,
            author: user1_id,
            content: BoardMessageBody::public(String::new(), "Message from user1".to_string()),
            time: SystemTime::now(),
        };

        let msg2 = MessageV1 {
            board_owner: owner_id,
            author: user2_id,
            content: BoardMessageBody::public(String::new(), "Message from user2".to_string()),
            time: SystemTime::now() + Duration::from_secs(1),
        };

        let auth_msg1 = AuthorizedMessageV1::new(msg1.clone(), &user1_sk);
        let auth_msg2 = AuthorizedMessageV1::new(msg2.clone(), &user2_sk);

        // Create a messages state with both messages
        let messages = MessagesV1 {
            messages: vec![auth_msg1.clone(), auth_msg2.clone()],
            ..Default::default()
        };

        // Verify authors are preserved
        assert_eq!(messages.messages.len(), 2);

        let stored_msg1 = &messages.messages[0];
        let stored_msg2 = &messages.messages[1];

        assert_eq!(
            stored_msg1.message.author, user1_id,
            "Message 1 author should be user1, but got {}",
            stored_msg1.message.author
        );
        assert_eq!(
            stored_msg2.message.author, user2_id,
            "Message 2 author should be user2, but got {}",
            stored_msg2.message.author
        );

        // Test that author IDs are different
        assert_ne!(user1_id, user2_id, "User IDs should be different");

        // Test Display implementation
        let user1_id_str = user1_id.to_string();
        let user2_id_str = user2_id.to_string();

        println!("User1 ID string: {}", user1_id_str);
        println!("User2 ID string: {}", user2_id_str);

        assert_ne!(
            user1_id_str, user2_id_str,
            "User ID strings should be different"
        );
    }

    #[test]
    fn test_edit_action() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let owner_id = MemberId::from(&verifying_key);
        let author_id = owner_id;

        // Create original message
        let original_msg = MessageV1 {
            board_owner: owner_id,
            author: author_id,
            time: SystemTime::now(),
            content: BoardMessageBody::public(String::new(), "Original content".to_string()),
        };
        let auth_original = AuthorizedMessageV1::new(original_msg, &signing_key);
        let original_id = auth_original.id();

        // Create edit action
        let edit_msg = MessageV1 {
            board_owner: owner_id,
            author: author_id,
            time: SystemTime::now() + Duration::from_secs(1),
            content: BoardMessageBody::edit(
                original_id.clone(),
                String::new(),
                "Edited content".to_string(),
            ),
        };
        let auth_edit = AuthorizedMessageV1::new(edit_msg, &signing_key);

        // Create messages state and rebuild
        let mut messages = MessagesV1 {
            messages: vec![auth_original.clone(), auth_edit],
            ..Default::default()
        };
        messages.rebuild_actions_state();

        // Verify edit was applied
        assert!(messages.is_edited(&original_id));
        let effective = messages.effective_text(&auth_original);
        assert_eq!(effective, Some("Edited content".to_string()));

        // Verify display_messages still shows the original message
        let display: Vec<_> = messages.display_messages().collect();
        assert_eq!(display.len(), 1);
    }

    #[test]
    fn test_edit_by_non_author_ignored() {
        let owner_sk = SigningKey::generate(&mut OsRng);
        let owner_vk = owner_sk.verifying_key();
        let owner_id = MemberId::from(&owner_vk);

        let other_sk = SigningKey::generate(&mut OsRng);
        let other_id = MemberId::from(&other_sk.verifying_key());

        // Create message by owner
        let original_msg = MessageV1 {
            board_owner: owner_id,
            author: owner_id,
            time: SystemTime::now(),
            content: BoardMessageBody::public(String::new(), "Original content".to_string()),
        };
        let auth_original = AuthorizedMessageV1::new(original_msg, &owner_sk);
        let original_id = auth_original.id();

        // Create edit action by OTHER user (should be ignored)
        let edit_msg = MessageV1 {
            board_owner: owner_id,
            author: other_id,
            time: SystemTime::now() + Duration::from_secs(1),
            content: BoardMessageBody::edit(
                original_id.clone(),
                String::new(),
                "Hacked content".to_string(),
            ),
        };
        let auth_edit = AuthorizedMessageV1::new(edit_msg, &other_sk);

        let mut messages = MessagesV1 {
            messages: vec![auth_original.clone(), auth_edit],
            ..Default::default()
        };
        messages.rebuild_actions_state();

        // Edit should be ignored - original content preserved
        assert!(!messages.is_edited(&original_id));
        let effective = messages.effective_text(&auth_original);
        assert_eq!(effective, Some("Original content".to_string()));
    }

    #[test]
    fn test_delete_action() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let owner_id = MemberId::from(&verifying_key);

        // Create original message
        let original_msg = MessageV1 {
            board_owner: owner_id,
            author: owner_id,
            time: SystemTime::now(),
            content: BoardMessageBody::public(String::new(), "Will be deleted".to_string()),
        };
        let auth_original = AuthorizedMessageV1::new(original_msg, &signing_key);
        let original_id = auth_original.id();

        // Create delete action
        let delete_msg = MessageV1 {
            board_owner: owner_id,
            author: owner_id,
            time: SystemTime::now() + Duration::from_secs(1),
            content: BoardMessageBody::delete(original_id.clone()),
        };
        let auth_delete = AuthorizedMessageV1::new(delete_msg, &signing_key);

        let mut messages = MessagesV1 {
            messages: vec![auth_original, auth_delete],
            ..Default::default()
        };
        messages.rebuild_actions_state();

        // Verify message is deleted
        assert!(messages.is_deleted(&original_id));

        // Verify display_messages excludes deleted message
        let display: Vec<_> = messages.display_messages().collect();
        assert_eq!(display.len(), 0);
    }

    #[test]
    fn test_reaction_action() {
        let user1_sk = SigningKey::generate(&mut OsRng);
        let user1_id = MemberId::from(&user1_sk.verifying_key());

        let user2_sk = SigningKey::generate(&mut OsRng);
        let user2_id = MemberId::from(&user2_sk.verifying_key());

        let owner_id = user1_id;

        // Create original message
        let original_msg = MessageV1 {
            board_owner: owner_id,
            author: user1_id,
            time: SystemTime::now(),
            content: BoardMessageBody::public(String::new(), "React to me!".to_string()),
        };
        let auth_original = AuthorizedMessageV1::new(original_msg, &user1_sk);
        let original_id = auth_original.id();

        // Create reaction from user2
        let reaction_msg = MessageV1 {
            board_owner: owner_id,
            author: user2_id,
            time: SystemTime::now() + Duration::from_secs(1),
            content: BoardMessageBody::reaction(original_id.clone(), "👍".to_string()),
        };
        let auth_reaction = AuthorizedMessageV1::new(reaction_msg, &user2_sk);

        // Create another reaction from user1
        let reaction_msg2 = MessageV1 {
            board_owner: owner_id,
            author: user1_id,
            time: SystemTime::now() + Duration::from_secs(2),
            content: BoardMessageBody::reaction(original_id.clone(), "👍".to_string()),
        };
        let auth_reaction2 = AuthorizedMessageV1::new(reaction_msg2, &user1_sk);

        let mut messages = MessagesV1 {
            messages: vec![auth_original, auth_reaction, auth_reaction2],
            ..Default::default()
        };
        messages.rebuild_actions_state();

        // Verify reactions
        let reactions = messages.reactions(&original_id).unwrap();
        let thumbs_up = reactions.get("👍").unwrap();
        assert_eq!(thumbs_up.len(), 2);
        assert!(thumbs_up.contains(&user1_id));
        assert!(thumbs_up.contains(&user2_id));
    }

    #[test]
    fn test_remove_reaction_action() {
        let user_sk = SigningKey::generate(&mut OsRng);
        let user_id = MemberId::from(&user_sk.verifying_key());
        let owner_id = user_id;

        // Create original message
        let original_msg = MessageV1 {
            board_owner: owner_id,
            author: user_id,
            time: SystemTime::now(),
            content: BoardMessageBody::public(String::new(), "Test message".to_string()),
        };
        let auth_original = AuthorizedMessageV1::new(original_msg, &user_sk);
        let original_id = auth_original.id();

        // Add reaction
        let reaction_msg = MessageV1 {
            board_owner: owner_id,
            author: user_id,
            time: SystemTime::now() + Duration::from_secs(1),
            content: BoardMessageBody::reaction(original_id.clone(), "❤️".to_string()),
        };
        let auth_reaction = AuthorizedMessageV1::new(reaction_msg, &user_sk);

        // Remove reaction
        let remove_msg = MessageV1 {
            board_owner: owner_id,
            author: user_id,
            time: SystemTime::now() + Duration::from_secs(2),
            content: BoardMessageBody::remove_reaction(original_id.clone(), "❤️".to_string()),
        };
        let auth_remove = AuthorizedMessageV1::new(remove_msg, &user_sk);

        let mut messages = MessagesV1 {
            messages: vec![auth_original, auth_reaction, auth_remove],
            ..Default::default()
        };
        messages.rebuild_actions_state();

        // Verify reaction was removed
        assert!(messages.reactions(&original_id).is_none());
    }

    #[test]
    fn test_action_on_deleted_message_ignored() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let owner_id = MemberId::from(&verifying_key);

        // Create original message
        let original_msg = MessageV1 {
            board_owner: owner_id,
            author: owner_id,
            time: SystemTime::now(),
            content: BoardMessageBody::public(String::new(), "Will be deleted".to_string()),
        };
        let auth_original = AuthorizedMessageV1::new(original_msg, &signing_key);
        let original_id = auth_original.id();

        // Delete it
        let delete_msg = MessageV1 {
            board_owner: owner_id,
            author: owner_id,
            time: SystemTime::now() + Duration::from_secs(1),
            content: BoardMessageBody::delete(original_id.clone()),
        };
        let auth_delete = AuthorizedMessageV1::new(delete_msg, &signing_key);

        // Try to edit deleted message (should be ignored)
        let edit_msg = MessageV1 {
            board_owner: owner_id,
            author: owner_id,
            time: SystemTime::now() + Duration::from_secs(2),
            content: BoardMessageBody::edit(
                original_id.clone(),
                String::new(),
                "Too late!".to_string(),
            ),
        };
        let auth_edit = AuthorizedMessageV1::new(edit_msg, &signing_key);

        let mut messages = MessagesV1 {
            messages: vec![auth_original, auth_delete, auth_edit],
            ..Default::default()
        };
        messages.rebuild_actions_state();

        // Message should be deleted, edit should be ignored
        assert!(messages.is_deleted(&original_id));
        assert!(!messages.is_edited(&original_id));
    }

    #[test]
    fn test_display_messages_filters_actions() {
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let owner_id = MemberId::from(&verifying_key);

        // Create regular message
        let msg1 = MessageV1 {
            board_owner: owner_id,
            author: owner_id,
            time: SystemTime::now(),
            content: BoardMessageBody::public(String::new(), "Hello".to_string()),
        };
        let auth_msg1 = AuthorizedMessageV1::new(msg1, &signing_key);
        let msg1_id = auth_msg1.id();

        // Create reaction (action message)
        let reaction_msg = MessageV1 {
            board_owner: owner_id,
            author: owner_id,
            time: SystemTime::now() + Duration::from_secs(1),
            content: BoardMessageBody::reaction(msg1_id, "👍".to_string()),
        };
        let auth_reaction = AuthorizedMessageV1::new(reaction_msg, &signing_key);

        // Create another regular message
        let msg2 = MessageV1 {
            board_owner: owner_id,
            author: owner_id,
            time: SystemTime::now() + Duration::from_secs(2),
            content: BoardMessageBody::public(String::new(), "World".to_string()),
        };
        let auth_msg2 = AuthorizedMessageV1::new(msg2, &signing_key);

        let mut messages = MessagesV1 {
            messages: vec![auth_msg1, auth_reaction, auth_msg2],
            ..Default::default()
        };
        messages.rebuild_actions_state();

        // display_messages should only return regular messages, not actions
        let display: Vec<_> = messages.display_messages().collect();
        assert_eq!(display.len(), 2);
        assert_eq!(
            display[0].message.content.as_public_string(),
            Some("Hello".to_string())
        );
        assert_eq!(
            display[1].message.content.as_public_string(),
            Some("World".to_string())
        );
    }

    #[test]
    fn test_edit_action_on_category_ignored() {
        // Edit actions (ACTION_TYPE_EDIT) should be ignored when targeting a category
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let owner_id = MemberId::from(&verifying_key);

        // Create a category
        let category_msg = MessageV1 {
            board_owner: owner_id,
            author: owner_id,
            time: SystemTime::now(),
            content: BoardMessageBody::category(
                "Test Category".to_string(),
                Some("Description".to_string()),
                None,
                "#ff0000".to_string(),
                None,
            ),
        };
        let auth_category = AuthorizedMessageV1::new(category_msg, &signing_key);
        let category_id = auth_category.id();

        // Try to edit the category using ACTION_TYPE_EDIT (wrong action type)
        let edit_msg = MessageV1 {
            board_owner: owner_id,
            author: owner_id,
            time: SystemTime::now() + Duration::from_secs(1),
            content: BoardMessageBody::edit(
                category_id.clone(),
                String::new(),
                "Hacked content".to_string(),
            ),
        };
        let auth_edit = AuthorizedMessageV1::new(edit_msg, &signing_key);

        let mut messages = MessagesV1 {
            messages: vec![auth_category, auth_edit],
            ..Default::default()
        };
        messages.rebuild_actions_state();

        // Edit should be ignored - category should NOT appear as edited
        assert!(!messages.is_edited(&category_id));
    }

    #[test]
    fn test_edit_category_action_on_regular_message_ignored() {
        // Edit category actions (ACTION_TYPE_EDIT_CATEGORY) should be ignored when targeting a regular message
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        let owner_id = MemberId::from(&verifying_key);

        // Create a regular text message
        let text_msg = MessageV1 {
            board_owner: owner_id,
            author: owner_id,
            time: SystemTime::now(),
            content: BoardMessageBody::public(String::new(), "Regular message".to_string()),
        };
        let auth_text = AuthorizedMessageV1::new(text_msg, &signing_key);
        let text_id = auth_text.id();

        // Try to edit the text message using ACTION_TYPE_EDIT_CATEGORY (wrong action type)
        let edit_msg = MessageV1 {
            board_owner: owner_id,
            author: owner_id,
            time: SystemTime::now() + Duration::from_secs(1),
            content: BoardMessageBody::edit_category(
                text_id.clone(),
                "Hacked name".to_string(),
                None,
                None,
                "#ff0000".to_string(),
            ),
        };
        let auth_edit = AuthorizedMessageV1::new(edit_msg, &signing_key);

        let mut messages = MessagesV1 {
            messages: vec![auth_text, auth_edit],
            ..Default::default()
        };
        messages.rebuild_actions_state();

        // Edit should be ignored - message should NOT appear as category-edited
        assert!(!messages.is_category_edited(&text_id));
    }
}
