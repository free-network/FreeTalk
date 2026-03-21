//! Message content types for client-side interpretation.
//!
//! The contract treats message content as opaque bytes with type/version tags.
//! This module defines the client-side content types that are encoded into those bytes.
//!
//! # Extensibility
//!
//! - New content types: Add new `CONTENT_TYPE_*` constant, no contract change needed
//! - New action types: Add new `ACTION_TYPE_*` constant, no contract change needed
//! - New fields on existing types: Just add them (old clients ignore unknown fields)
//! - Breaking format changes: Bump the version constant for that type

use crate::board_state::message::MessageId;
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

/// Content type constants
pub const CONTENT_TYPE_TEXT: u32 = 1;
pub const CONTENT_TYPE_ACTION: u32 = 2;
pub const CONTENT_TYPE_REPLY: u32 = 3;
pub const CONTENT_TYPE_CATEGORY: u32 = 4;
// Future: CONTENT_TYPE_BLOB = 5, CONTENT_TYPE_POLL = 6, etc.

/// Current version for text content
pub const TEXT_CONTENT_VERSION: u32 = 1;

/// Current version for action content
pub const ACTION_CONTENT_VERSION: u32 = 1;

/// Current version for reply content
pub const REPLY_CONTENT_VERSION: u32 = 1;

/// Current version for category content
pub const CATEGORY_CONTENT_VERSION: u32 = 1;

/// Action type constants
pub const ACTION_TYPE_EDIT: u32 = 1;
pub const ACTION_TYPE_DELETE: u32 = 2;
pub const ACTION_TYPE_REACTION: u32 = 3;
pub const ACTION_TYPE_REMOVE_REACTION: u32 = 4;
pub const ACTION_TYPE_EDIT_CATEGORY: u32 = 5;
// Future: ACTION_TYPE_PIN = 6, ACTION_TYPE_REPLY = 7, etc.

/// Text message content (content_type = 1)
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct TextContentV1 {
    pub title: String,
    pub content: String,
}

impl TextContentV1 {
    pub fn new(title: String, content: String) -> Self {
        Self { title, content }
    }

    /// Encode to CBOR bytes
    pub fn encode(&self) -> Vec<u8> {
        encode_cbor(self)
    }

    /// Decode from CBOR bytes
    pub fn decode(data: &[u8]) -> Result<Self, String> {
        decode_cbor(data, "TextContentV1")
    }
}

/// Encode a value to CBOR bytes
fn encode_cbor<T: Serialize>(value: &T) -> Vec<u8> {
    let mut data = Vec::new();
    ciborium::into_writer(value, &mut data).expect("CBOR serialization should not fail");
    data
}

/// Decode a value from CBOR bytes
fn decode_cbor<T: serde::de::DeserializeOwned>(data: &[u8], type_name: &str) -> Result<T, String> {
    ciborium::from_reader(data).map_err(|e| format!("Failed to decode {}: {}", type_name, e))
}

/// Action message content (content_type = 2)
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ActionContentV1 {
    /// Type of action (ACTION_TYPE_* constants)
    pub action_type: u32,
    /// Target message ID for the action
    pub target: MessageId,
    /// Action-specific payload (CBOR-encoded)
    pub payload: Vec<u8>,
}

impl ActionContentV1 {
    /// Create an edit action
    pub fn edit(target: MessageId, new_title: String, new_text: String) -> Self {
        Self {
            action_type: ACTION_TYPE_EDIT,
            target,
            payload: encode_cbor(&EditPayload {
                new_title,
                new_text,
            }),
        }
    }

    /// Create a delete action
    pub fn delete(target: MessageId) -> Self {
        Self {
            action_type: ACTION_TYPE_DELETE,
            target,
            payload: Vec::new(),
        }
    }

    /// Create a reaction action
    pub fn reaction(target: MessageId, emoji: String) -> Self {
        Self {
            action_type: ACTION_TYPE_REACTION,
            target,
            payload: encode_cbor(&ReactionPayload { emoji }),
        }
    }

    /// Create a remove reaction action
    pub fn remove_reaction(target: MessageId, emoji: String) -> Self {
        Self {
            action_type: ACTION_TYPE_REMOVE_REACTION,
            target,
            payload: encode_cbor(&ReactionPayload { emoji }),
        }
    }

    /// Create an edit category action
    pub fn edit_category(
        target: MessageId,
        new_name: String,
        new_description: Option<String>,
        new_icon: Option<String>,
        new_color: String,
    ) -> Self {
        Self {
            action_type: ACTION_TYPE_EDIT_CATEGORY,
            target,
            payload: encode_cbor(&CategoryEditPayload {
                new_name,
                new_description,
                new_icon,
                new_color,
            }),
        }
    }

    /// Encode to CBOR bytes
    pub fn encode(&self) -> Vec<u8> {
        encode_cbor(self)
    }

    /// Decode from CBOR bytes
    pub fn decode(data: &[u8]) -> Result<Self, String> {
        decode_cbor(data, "ActionContentV1")
    }

    /// Get the edit payload if this is an edit action
    pub fn edit_payload(&self) -> Option<EditPayload> {
        if self.action_type == ACTION_TYPE_EDIT {
            ciborium::from_reader(&self.payload[..]).ok()
        } else {
            None
        }
    }

    /// Get the reaction payload if this is a reaction or remove_reaction action
    pub fn reaction_payload(&self) -> Option<ReactionPayload> {
        if self.action_type == ACTION_TYPE_REACTION
            || self.action_type == ACTION_TYPE_REMOVE_REACTION
        {
            ciborium::from_reader(&self.payload[..]).ok()
        } else {
            None
        }
    }

    /// Get the category edit payload if this is an edit_category action
    pub fn category_edit_payload(&self) -> Option<CategoryEditPayload> {
        if self.action_type == ACTION_TYPE_EDIT_CATEGORY {
            ciborium::from_reader(&self.payload[..]).ok()
        } else {
            None
        }
    }
}

/// Payload for edit actions
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct EditPayload {
    pub new_title: String,
    pub new_text: String,
}

/// Payload for reaction actions
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ReactionPayload {
    pub emoji: String,
}

/// Payload for category edit actions
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct CategoryEditPayload {
    pub new_name: String,
    pub new_description: Option<String>,
    pub new_icon: Option<String>,
    pub new_color: String,
}

/// Reply message content (content_type = 3)
///
/// A reply references a target message and includes a snapshot of the target's
/// author name and content preview so that the reply remains meaningful even if
/// the target message is later deleted or scrolled out of the recent window.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ReplyContentV1 {
    pub title: String,
    pub content: String,
    pub target_message_id: MessageId,
    pub target_author_name: String,
    /// Snapshot of the target message content (~100 chars)
    pub target_content_preview: String,
}

impl ReplyContentV1 {
    pub fn new(
        title: String,
        content: String,
        target_message_id: MessageId,
        target_author_name: String,
        target_content_preview: String,
    ) -> Self {
        Self {
            title,
            content,
            target_message_id,
            target_author_name,
            target_content_preview,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        encode_cbor(self)
    }

    pub fn decode(data: &[u8]) -> Result<Self, String> {
        decode_cbor(data, "ReplyContentV1")
    }
}

/// Default color for categories (used for backwards compatibility)
pub const DEFAULT_CATEGORY_COLOR: &str = "#6366f1";

/// Category content (content_type = 4)
///
/// Categories organize posts into hierarchical groups.
/// Only board owners and admins can create categories.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct CategoryContentV1 {
    /// Category name/title
    pub name: String,
    /// Optional description
    pub description: Option<String>,
    /// Optional icon (emoji)
    pub icon: Option<String>,
    /// Color for UI display (hex like "#6366f1")
    pub color: String,
    /// Parent category ID (None for top-level categories)
    pub parent_category_id: Option<MessageId>,
}

impl CategoryContentV1 {
    /// Create a new category. Color is required.
    pub fn new(name: String, description: Option<String>, color: String) -> Self {
        Self {
            name,
            description,
            icon: None,
            color,
            parent_category_id: None,
        }
    }

    /// Set the icon, enforcing that it is exactly one grapheme cluster.
    /// If the input contains multiple graphemes, only the first is used.
    /// If the input is empty, no icon is set.
    pub fn with_icon(mut self, icon: String) -> Self {
        self.icon = normalize_icon(&icon);
        self
    }

    pub fn with_parent(mut self, parent_id: MessageId) -> Self {
        self.parent_category_id = Some(parent_id);
        self
    }

    pub fn encode(&self) -> Vec<u8> {
        encode_cbor(self)
    }

    pub fn decode(data: &[u8]) -> Result<Self, String> {
        decode_cbor(data, "CategoryContentV1")
    }
}

/// Normalize an icon string to exactly one grapheme cluster.
/// Returns None if the input is empty, otherwise returns the first grapheme.
pub fn normalize_icon(icon: &str) -> Option<String> {
    let trimmed = icon.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed.graphemes(true).next().map(|g| g.to_string())
}

/// Validate that an icon string is exactly one grapheme cluster.
/// Returns Ok(()) if valid, Err with message if invalid.
pub fn validate_icon(icon: &str) -> Result<(), String> {
    let trimmed = icon.trim();
    if trimmed.is_empty() {
        return Ok(()); // Empty is allowed (no icon)
    }
    let grapheme_count = trimmed.graphemes(true).count();
    if grapheme_count != 1 {
        return Err(format!(
            "Icon must be exactly 1 character (emoji), got {} characters",
            grapheme_count
        ));
    }
    Ok(())
}

/// Decoded message content for client-side processing
#[derive(Clone, PartialEq, Debug)]
pub enum DecodedContent {
    /// Text message
    Text(TextContentV1),
    /// Action on another message
    Action(ActionContentV1),
    /// Reply to another message
    Reply(ReplyContentV1),
    /// Category (for organizing posts)
    Category(CategoryContentV1),
    /// Unknown content type - preserved for round-tripping but displayed as placeholder
    Unknown {
        content_type: u32,
        content_version: u32,
    },
}

impl DecodedContent {
    /// Check if this is an action
    pub fn is_action(&self) -> bool {
        matches!(self, Self::Action(_))
    }

    /// Get the target message ID if this is an action
    pub fn target_id(&self) -> Option<&MessageId> {
        match self {
            Self::Action(action) => Some(&action.target),
            _ => None,
        }
    }

    /// Get the text content if this is a text, reply, or category message
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(&text.content),
            Self::Reply(reply) => Some(&reply.content),
            Self::Category(cat) => cat.description.as_deref(),
            _ => None,
        }
    }

    /// Check if this is a category
    pub fn is_category(&self) -> bool {
        matches!(self, Self::Category(_))
    }

    /// Get the category content if this is a category
    pub fn as_category(&self) -> Option<&CategoryContentV1> {
        match self {
            Self::Category(cat) => Some(cat),
            _ => None,
        }
    }

    /// Get a display string for this content
    pub fn to_display_string(&self) -> String {
        match self {
            Self::Text(text) => text.content.clone(),
            Self::Reply(reply) => reply.content.clone(),
            Self::Action(action) => match action.action_type {
                ACTION_TYPE_EDIT => format!("[Edit of message {}]", action.target),
                ACTION_TYPE_DELETE => format!("[Delete of message {}]", action.target),
                ACTION_TYPE_REACTION => {
                    let emoji = action
                        .reaction_payload()
                        .map(|p| p.emoji)
                        .unwrap_or_else(|| "?".to_string());
                    format!("[Reaction {} to {}]", emoji, action.target)
                }
                ACTION_TYPE_REMOVE_REACTION => {
                    let emoji = action
                        .reaction_payload()
                        .map(|p| p.emoji)
                        .unwrap_or_else(|| "?".to_string());
                    format!("[Remove reaction {} from {}]", emoji, action.target)
                }
                _ => format!(
                    "[Unknown action type {} on {}]",
                    action.action_type, action.target
                ),
            },
            Self::Category(cat) => {
                let icon = cat.icon.as_deref().unwrap_or("📁");
                format!("{} {}", icon, cat.name)
            }
            Self::Unknown {
                content_type,
                content_version,
            } => format!(
                "[Unsupported message type {}.{} - please upgrade]",
                content_type, content_version
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use freenet_scaffold::util::fast_hash;

    fn test_message_id() -> MessageId {
        MessageId(fast_hash(&[1, 2, 3, 4]))
    }

    #[test]
    fn test_text_content_roundtrip() {
        let content = TextContentV1::new("Title".to_string(), "Hello, world!".to_string());
        let encoded = content.encode();
        let decoded = TextContentV1::decode(&encoded).unwrap();
        assert_eq!(content, decoded);
    }

    #[test]
    fn test_edit_action_roundtrip() {
        let action =
            ActionContentV1::edit(test_message_id(), String::new(), "New text".to_string());
        let encoded = action.encode();
        let decoded = ActionContentV1::decode(&encoded).unwrap();
        assert_eq!(action, decoded);

        let payload = decoded.edit_payload().unwrap();
        assert_eq!(payload.new_text, "New text");
    }

    #[test]
    fn test_delete_action_roundtrip() {
        let action = ActionContentV1::delete(test_message_id());
        let encoded = action.encode();
        let decoded = ActionContentV1::decode(&encoded).unwrap();
        assert_eq!(action, decoded);
        assert_eq!(decoded.action_type, ACTION_TYPE_DELETE);
    }

    #[test]
    fn test_reaction_action_roundtrip() {
        let action = ActionContentV1::reaction(test_message_id(), "👍".to_string());
        let encoded = action.encode();
        let decoded = ActionContentV1::decode(&encoded).unwrap();
        assert_eq!(action, decoded);

        let payload = decoded.reaction_payload().unwrap();
        assert_eq!(payload.emoji, "👍");
    }

    #[test]
    fn test_remove_reaction_action_roundtrip() {
        let action = ActionContentV1::remove_reaction(test_message_id(), "❤️".to_string());
        let encoded = action.encode();
        let decoded = ActionContentV1::decode(&encoded).unwrap();
        assert_eq!(action, decoded);

        let payload = decoded.reaction_payload().unwrap();
        assert_eq!(payload.emoji, "❤️");
    }

    #[test]
    fn test_reply_content_roundtrip() {
        let reply = ReplyContentV1::new(
            "Re: Hello".to_string(),
            "I agree!".to_string(),
            test_message_id(),
            "Alice".to_string(),
            "The original message text here...".to_string(),
        );
        let encoded = reply.encode();
        let decoded = ReplyContentV1::decode(&encoded).unwrap();
        assert_eq!(reply, decoded);

        // Verify DecodedContent::Reply returns content via as_text()
        let dc = DecodedContent::Reply(reply.clone());
        assert_eq!(dc.as_text(), Some("I agree!"));
        assert_eq!(dc.to_display_string(), "I agree!");
        assert!(!dc.is_action());
    }

    #[test]
    fn test_decoded_content_display() {
        let text = DecodedContent::Text(TextContentV1::new(String::new(), "Hello".to_string()));
        assert_eq!(text.to_display_string(), "Hello");

        let unknown = DecodedContent::Unknown {
            content_type: 99,
            content_version: 1,
        };
        assert!(unknown.to_display_string().contains("Unsupported"));
    }

    #[test]
    fn test_category_content_roundtrip() {
        let category = CategoryContentV1::new("General".to_string(), Some("General discussion".to_string()), "#6366f1".to_string())
            .with_icon("💬".to_string());
        let encoded = category.encode();
        let decoded = CategoryContentV1::decode(&encoded).unwrap();
        assert_eq!(category, decoded);
        assert_eq!(decoded.name, "General");
        assert_eq!(decoded.icon, Some("💬".to_string()));
        assert_eq!(decoded.color, "#6366f1".to_string());
    }

    #[test]
    fn test_category_with_parent() {
        let parent_id = test_message_id();
        let category = CategoryContentV1::new("Subcategory".to_string(), None, "#ef4444".to_string())
            .with_parent(parent_id.clone());
        let encoded = category.encode();
        let decoded = CategoryContentV1::decode(&encoded).unwrap();
        assert_eq!(decoded.parent_category_id, Some(parent_id));
    }

    #[test]
    fn test_decoded_content_category() {
        let cat = CategoryContentV1::new("Test".to_string(), Some("Description".to_string()), "#22c55e".to_string())
            .with_icon("🎯".to_string());
        let dc = DecodedContent::Category(cat.clone());
        assert!(dc.is_category());
        assert_eq!(dc.as_category(), Some(&cat));
        assert_eq!(dc.to_display_string(), "🎯 Test");
    }
}
