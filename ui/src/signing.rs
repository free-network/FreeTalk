//! Signing API for delegate-based signing operations.
//!
//! This module provides async wrapper functions that send signing requests
//! to the chat delegate and wait for responses. The delegate holds the signing
//! keys and performs all signing operations, so private keys never leave the delegate.
//!
//! The module also provides fallback functionality that signs locally if the
//! delegate signing fails, for backwards compatibility during migration.

use crate::components::app::chat_delegate::{generate_request_id, send_delegate_request};
use dioxus::logger::tracing::{info, warn};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use river_core::chat_delegate::{BoardKey, ChatDelegateRequestMsg, ChatDelegateResponseMsg};

/// Store a signing key in the delegate for a board.
///
/// This should be called when creating a new board or when migrating
/// an existing board's signing key to the delegate.
pub async fn store_signing_key(
    board_key: BoardKey,
    signing_key: &SigningKey,
) -> Result<(), String> {
    let request = ChatDelegateRequestMsg::StoreSigningKey {
        board_key,
        signing_key_bytes: signing_key.to_bytes(),
    };

    match send_delegate_request(request).await {
        Ok(ChatDelegateResponseMsg::StoreSigningKeyResponse { result, .. }) => result,
        Ok(other) => Err(format!("Unexpected response: {:?}", other)),
        Err(e) => Err(e),
    }
}

/// Get the public key for a board from the delegate.
///
/// Returns the VerifyingKey if the signing key exists, None otherwise.
pub async fn get_public_key(board_key: BoardKey) -> Result<Option<VerifyingKey>, String> {
    let request = ChatDelegateRequestMsg::GetPublicKey { board_key };

    match send_delegate_request(request).await {
        Ok(ChatDelegateResponseMsg::GetPublicKeyResponse { public_key, .. }) => {
            if let Some(pk_bytes) = public_key {
                let vk = VerifyingKey::from_bytes(&pk_bytes)
                    .map_err(|e| format!("Invalid public key: {}", e))?;
                Ok(Some(vk))
            } else {
                Ok(None)
            }
        }
        Ok(other) => Err(format!("Unexpected response: {:?}", other)),
        Err(e) => Err(e),
    }
}

/// Sign a message (MessageV1).
pub async fn sign_message(
    board_key: BoardKey,
    message_bytes: Vec<u8>,
) -> Result<Signature, String> {
    let request = ChatDelegateRequestMsg::SignMessage {
        board_key,
        request_id: generate_request_id(),
        message_bytes,
    };

    extract_signature(send_delegate_request(request).await)
}

/// Sign a member invitation (Member).
pub async fn sign_member(board_key: BoardKey, member_bytes: Vec<u8>) -> Result<Signature, String> {
    let request = ChatDelegateRequestMsg::SignMember {
        board_key,
        request_id: generate_request_id(),
        member_bytes,
    };

    extract_signature(send_delegate_request(request).await)
}

/// Sign a ban (BanV1).
pub async fn sign_ban(board_key: BoardKey, ban_bytes: Vec<u8>) -> Result<Signature, String> {
    let request = ChatDelegateRequestMsg::SignBan {
        board_key,
        request_id: generate_request_id(),
        ban_bytes,
    };

    extract_signature(send_delegate_request(request).await)
}

/// Sign a board configuration (Configuration).
pub async fn sign_config(board_key: BoardKey, config_bytes: Vec<u8>) -> Result<Signature, String> {
    let request = ChatDelegateRequestMsg::SignConfig {
        board_key,
        request_id: generate_request_id(),
        config_bytes,
    };

    extract_signature(send_delegate_request(request).await)
}

/// Sign member info (MemberInfo).
pub async fn sign_member_info(
    board_key: BoardKey,
    member_info_bytes: Vec<u8>,
) -> Result<Signature, String> {
    let request = ChatDelegateRequestMsg::SignMemberInfo {
        board_key,
        request_id: generate_request_id(),
        member_info_bytes,
    };

    extract_signature(send_delegate_request(request).await)
}

/// Sign a secret version record (SecretVersionRecordV1).
pub async fn sign_secret_version(
    board_key: BoardKey,
    record_bytes: Vec<u8>,
) -> Result<Signature, String> {
    let request = ChatDelegateRequestMsg::SignSecretVersion {
        board_key,
        request_id: generate_request_id(),
        record_bytes,
    };

    extract_signature(send_delegate_request(request).await)
}

/// Sign an encrypted secret for member (EncryptedSecretForMemberV1).
pub async fn sign_encrypted_secret(
    board_key: BoardKey,
    secret_bytes: Vec<u8>,
) -> Result<Signature, String> {
    let request = ChatDelegateRequestMsg::SignEncryptedSecret {
        board_key,
        request_id: generate_request_id(),
        secret_bytes,
    };

    extract_signature(send_delegate_request(request).await)
}

/// Sign a board upgrade (BoardUpgrade).
pub async fn sign_upgrade(
    board_key: BoardKey,
    upgrade_bytes: Vec<u8>,
) -> Result<Signature, String> {
    let request = ChatDelegateRequestMsg::SignUpgrade {
        board_key,
        request_id: generate_request_id(),
        upgrade_bytes,
    };

    extract_signature(send_delegate_request(request).await)
}

/// Sign an admin authorization (Admin).
pub async fn sign_admin(board_key: BoardKey, admin_bytes: Vec<u8>) -> Result<Signature, String> {
    let request = ChatDelegateRequestMsg::SignAdmin {
        board_key,
        request_id: generate_request_id(),
        admin_bytes,
    };

    extract_signature(send_delegate_request(request).await)
}

/// Extract a signature from a delegate response.
fn extract_signature(
    response: Result<ChatDelegateResponseMsg, String>,
) -> Result<Signature, String> {
    match response {
        Ok(ChatDelegateResponseMsg::SignResponse { signature, .. }) => match signature {
            Ok(sig_bytes) => {
                if sig_bytes.len() != 64 {
                    return Err(format!(
                        "Invalid signature length: {} bytes (expected 64)",
                        sig_bytes.len()
                    ));
                }
                let sig_array: [u8; 64] = sig_bytes
                    .try_into()
                    .map_err(|_| "Failed to convert signature bytes to array".to_string())?;
                Ok(Signature::from_bytes(&sig_array))
            }
            Err(e) => Err(e),
        },
        Ok(other) => Err(format!("Unexpected response: {:?}", other)),
        Err(e) => Err(e),
    }
}

// ============================================================================
// Migration and Fallback Functions
// ============================================================================

/// Migrate a signing key to the delegate if not already present.
///
/// Returns true if migration was successful or key already exists in delegate.
/// Returns false if migration failed (fallback to local signing should be used).
pub async fn migrate_signing_key(board_key: BoardKey, signing_key: &SigningKey) -> bool {
    // Check if key already exists in delegate
    match get_public_key(board_key).await {
        Ok(Some(existing_vk)) => {
            // Verify it matches our key
            if existing_vk == signing_key.verifying_key() {
                info!("Signing key already migrated to delegate for board");
                return true;
            } else {
                // Delegate has a stale key (e.g. from before re-invitation).
                // Overwrite it so delegate signing produces valid signatures.
                warn!("Delegate has stale key for board - overwriting with current key");
            }
        }
        Ok(None) => {
            // Key not in delegate, try to store it
            info!("Migrating signing key to delegate for board");
        }
        Err(e) => {
            warn!(
                "Failed to check delegate for existing key: {} - will try to store",
                e
            );
        }
    }

    // Store the key
    match store_signing_key(board_key, signing_key).await {
        Ok(()) => {
            // Verify the key was stored correctly
            match get_public_key(board_key).await {
                Ok(Some(stored_vk)) if stored_vk == signing_key.verifying_key() => {
                    info!("Successfully migrated signing key to delegate");
                    true
                }
                Ok(Some(_)) => {
                    warn!("Stored key doesn't match - using local signing");
                    false
                }
                Ok(None) => {
                    warn!("Key not found after storing - using local signing");
                    false
                }
                Err(e) => {
                    warn!("Failed to verify stored key: {} - using local signing", e);
                    false
                }
            }
        }
        Err(e) => {
            warn!(
                "Failed to store signing key in delegate: {} - using local signing",
                e
            );
            false
        }
    }
}

/// Sign message bytes with delegate, falling back to local signing if delegate fails.
pub async fn sign_message_with_fallback(
    board_key: BoardKey,
    message_bytes: Vec<u8>,
    fallback_key: &SigningKey,
) -> Signature {
    match sign_message(board_key, message_bytes.clone()).await {
        Ok(sig) => sig,
        Err(e) => {
            warn!("Delegate signing failed, using fallback: {}", e);
            fallback_key.sign(&message_bytes)
        }
    }
}

/// Sign member bytes with delegate, falling back to local signing if delegate fails.
pub async fn sign_member_with_fallback(
    board_key: BoardKey,
    member_bytes: Vec<u8>,
    fallback_key: &SigningKey,
) -> Signature {
    match sign_member(board_key, member_bytes.clone()).await {
        Ok(sig) => sig,
        Err(e) => {
            warn!("Delegate signing failed, using fallback: {}", e);
            fallback_key.sign(&member_bytes)
        }
    }
}

/// Sign ban bytes with delegate, falling back to local signing if delegate fails.
pub async fn sign_ban_with_fallback(
    board_key: BoardKey,
    ban_bytes: Vec<u8>,
    fallback_key: &SigningKey,
) -> Signature {
    match sign_ban(board_key, ban_bytes.clone()).await {
        Ok(sig) => sig,
        Err(e) => {
            warn!("Delegate signing failed, using fallback: {}", e);
            fallback_key.sign(&ban_bytes)
        }
    }
}

/// Sign config bytes with delegate, falling back to local signing if delegate fails.
pub async fn sign_config_with_fallback(
    board_key: BoardKey,
    config_bytes: Vec<u8>,
    fallback_key: &SigningKey,
) -> Signature {
    match sign_config(board_key, config_bytes.clone()).await {
        Ok(sig) => sig,
        Err(e) => {
            warn!("Delegate signing failed, using fallback: {}", e);
            fallback_key.sign(&config_bytes)
        }
    }
}

/// Sign member info bytes with delegate, falling back to local signing if delegate fails.
pub async fn sign_member_info_with_fallback(
    board_key: BoardKey,
    member_info_bytes: Vec<u8>,
    fallback_key: &SigningKey,
) -> Signature {
    match sign_member_info(board_key, member_info_bytes.clone()).await {
        Ok(sig) => sig,
        Err(e) => {
            warn!("Delegate signing failed, using fallback: {}", e);
            fallback_key.sign(&member_info_bytes)
        }
    }
}

/// Sign secret version record bytes with delegate, falling back to local signing if delegate fails.
pub async fn sign_secret_version_with_fallback(
    board_key: BoardKey,
    record_bytes: Vec<u8>,
    fallback_key: &SigningKey,
) -> Signature {
    match sign_secret_version(board_key, record_bytes.clone()).await {
        Ok(sig) => sig,
        Err(e) => {
            warn!("Delegate signing failed, using fallback: {}", e);
            fallback_key.sign(&record_bytes)
        }
    }
}

/// Sign encrypted secret bytes with delegate, falling back to local signing if delegate fails.
pub async fn sign_encrypted_secret_with_fallback(
    board_key: BoardKey,
    secret_bytes: Vec<u8>,
    fallback_key: &SigningKey,
) -> Signature {
    match sign_encrypted_secret(board_key, secret_bytes.clone()).await {
        Ok(sig) => sig,
        Err(e) => {
            warn!("Delegate signing failed, using fallback: {}", e);
            fallback_key.sign(&secret_bytes)
        }
    }
}

/// Sign upgrade bytes with delegate, falling back to local signing if delegate fails.
pub async fn sign_upgrade_with_fallback(
    board_key: BoardKey,
    upgrade_bytes: Vec<u8>,
    fallback_key: &SigningKey,
) -> Signature {
    match sign_upgrade(board_key, upgrade_bytes.clone()).await {
        Ok(sig) => sig,
        Err(e) => {
            warn!("Delegate signing failed, using fallback: {}", e);
            fallback_key.sign(&upgrade_bytes)
        }
    }
}

/// Sign admin bytes with delegate, falling back to local signing if delegate fails.
pub async fn sign_admin_with_fallback(
    board_key: BoardKey,
    admin_bytes: Vec<u8>,
    fallback_key: &SigningKey,
) -> Signature {
    match sign_admin(board_key, admin_bytes.clone()).await {
        Ok(sig) => sig,
        Err(e) => {
            warn!("Delegate signing failed, using fallback: {}", e);
            fallback_key.sign(&admin_bytes)
        }
    }
}
