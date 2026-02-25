use crate::components::app::freenet_api::freenet_synchronizer::SynchronizerMessage;
use crate::components::app::{Route, NEEDS_SYNC, PENDING_INVITES, BOARDS, SYNCHRONIZER};
use crate::components::members::Invitation;
use crate::invites::{PendingBoardJoin, PendingBoardStatus};
use crate::board_data::Boards;
use dioxus::logger::tracing::{error, info};
use dioxus::prelude::*;
use ed25519_dalek::VerifyingKey;
use river_core::board_state::member::MemberId;
use wasm_bindgen::JsCast;

/// Main component for the invitation modal
#[component]
pub fn ReceiveInvitationModal(invitation: Invitation) -> Element {
    let nav = navigator();
    let board_key = invitation.board;

    // Listen for custom events from the FreenetSynchronizer
    use_effect(move || {
        let window = web_sys::window().expect("No window found");
        let closure = wasm_bindgen::closure::Closure::wrap(Box::new(
            move |event: web_sys::CustomEvent| {
                let detail = event.detail();
                if let Some(key_hex) = detail.as_string() {
                    info!("Received invitation accepted event with key: {}", key_hex);

                    // Convert hex string back to bytes
                    let mut bytes = Vec::new();
                    for i in 0..(key_hex.len() / 2) {
                        let byte_str = &key_hex[i * 2..(i + 1) * 2];
                        if let Ok(byte) = u8::from_str_radix(byte_str, 16) {
                            bytes.push(byte);
                        }
                    }

                    // Try to convert bytes to VerifyingKey
                    if bytes.len() == 32 {
                        let mut array = [0u8; 32];
                        array.copy_from_slice(&bytes);

                        if let Ok(key) = VerifyingKey::from_bytes(&array) {
                            // Use with_mut for atomic update
                            PENDING_INVITES.with_mut(|pending| {
                                if let Some(join) = pending.map.get_mut(&key) {
                                    join.status = PendingBoardStatus::Subscribed;
                                    info!(
                                        "Updated pending invitation status to Subscribed for key: {:?}",
                                        key
                                    );
                                }
                            });
                        }
                    }
                }
            },
        )
            as Box<dyn FnMut(web_sys::CustomEvent)>);

        window
            .add_event_listener_with_callback(
                "river-invitation-accepted",
                closure.as_ref().unchecked_ref(),
            )
            .expect("Failed to add event listener");

        closure.forget(); // Prevent closure from being dropped
    });

    // Check if invitation is already subscribed and navigate away
    let pending_status = PENDING_INVITES
        .read()
        .map
        .get(&board_key)
        .map(|join| join.status.clone());

    if matches!(pending_status, Some(PendingBoardStatus::Subscribed)) {
        // Board is ready, navigate to it
        let board_id = bs58::encode(board_key.as_bytes()).into_string();
        nav.push(Route::Posts { board_id });
        return rsx! {};
    }

    rsx! {
        // Modal backdrop - no click dismiss to prevent accidental invitation loss
        div {
            class: "fixed inset-0 z-50 flex items-center justify-center",
            // Overlay (non-dismissable)
            div {
                class: "absolute inset-0 bg-black/50",
            }
            // Modal content
            div {
                class: "relative z-10 w-full max-w-md mx-4 bg-panel rounded-xl shadow-xl border border-border",
                div {
                    class: "p-6",
                    h1 { class: "text-xl font-semibold text-text mb-4", "Invitation Received" }
                    {render_invitation_content(invitation.clone())}
                }
            }
        }
    }
}

/// Renders the content of the invitation modal based on the invitation data
fn render_invitation_content(inv: Invitation) -> Element {
    let pending_invites = PENDING_INVITES.read();
    let pending_status = pending_invites.map.get(&inv.board).map(|join| &join.status);

    match pending_status {
        Some(PendingBoardStatus::PendingSubscription) => render_pending_subscription_state(),
        Some(PendingBoardStatus::Subscribing) => render_subscribing_state(),
        Some(PendingBoardStatus::Error(e)) => render_error_state(e, inv.board),
        Some(PendingBoardStatus::Subscribed) => {
            // Board subscribed and retrieved successfully, navigate to it
            render_subscribed_state(inv.board)
        }
        None => render_invitation_options(inv),
    }
}

/// Renders the state when waiting to subscribe to board data
fn render_pending_subscription_state() -> Element {
    rsx! {
        div {
            class: "text-center py-4",
            p { class: "mb-4 text-text", "Preparing to subscribe to board..." }
            div { class: "w-full h-2 bg-surface rounded-full overflow-hidden",
                div { class: "h-full bg-accent animate-pulse w-1/2" }
            }
        }
    }
}

/// Renders the loading state when subscribing to board data
fn render_subscribing_state() -> Element {
    rsx! {
        div {
            class: "text-center py-4",
            p { class: "mb-4 text-text", "Subscribing to board..." }
            div { class: "w-full h-2 bg-surface rounded-full overflow-hidden",
                div { class: "h-full bg-blue-500 animate-pulse w-2/3" }
            }
        }
    }
}

/// Renders the error state when board retrieval fails
fn render_error_state(error: &str, board_key: VerifyingKey) -> Element {
    rsx! {
        div {
            class: "bg-red-500/10 border border-red-500/20 rounded-lg p-4",
            p { class: "mb-4 text-red-400", "Failed to retrieve board: {error}" }
            div {
                class: "flex gap-3",
                button {
                    class: "px-4 py-2 bg-accent hover:bg-accent-hover text-white font-medium rounded-lg transition-colors",
                    autofocus: true,
                    onmounted: move |cx| {
                        let element = cx.data();
                        wasm_bindgen_futures::spawn_local(async move {
                            let _ = element.set_focus(true).await;
                        });
                    },
                    onclick: move |_| {
                        // Reset to PendingSubscription so the synchronizer retries
                        PENDING_INVITES.with_mut(|pending| {
                            if let Some(join) = pending.map.get_mut(&board_key) {
                                join.status = PendingBoardStatus::PendingSubscription;
                            }
                        });
                    },
                    "Retry"
                }
                button {
                    class: "px-4 py-2 bg-surface hover:bg-surface-hover text-text rounded-lg transition-colors",
                    onclick: move |_| {
                        PENDING_INVITES.write().map.remove(&board_key);
                        navigator().push(Route::Home);
                    },
                    "Dismiss"
                }
            }
        }
    }
}

/// Renders the state when board is successfully subscribed and retrieved
fn render_subscribed_state(board_key: VerifyingKey) -> Element {
    let board_id = bs58::encode(board_key.as_bytes()).into_string();
    navigator().push(Route::Posts { board_id });
    rsx! {}
}

/// Renders the invitation options based on the user's membership status
fn render_invitation_options(inv: Invitation) -> Element {
    let (current_key_is_member, invited_member_exists) =
        check_membership_status(&inv, &BOARDS.read());

    if current_key_is_member {
        render_already_member()
    } else if invited_member_exists {
        render_restore_access_option(inv)
    } else {
        render_new_invitation(inv)
    }
}

/// Checks the membership status of the user in the board
fn check_membership_status(inv: &Invitation, current_boards: &Boards) -> (bool, bool) {
    if let Some(board_data) = current_boards.map.get(&inv.board) {
        let user_vk = inv.invitee_signing_key.verifying_key();
        let current_key_is_member = user_vk == board_data.owner_vk
            || board_data
                .board_state
                .members
                .members
                .iter()
                .any(|m| m.member.member_vk == user_vk);
        let invited_member_exists = board_data
            .board_state
            .members
            .members
            .iter()
            .any(|m| m.member.member_vk == inv.invitee.member.member_vk);
        (current_key_is_member, invited_member_exists)
    } else {
        (false, false)
    }
}

/// Renders the UI when the user is already a member of the board
fn render_already_member() -> Element {
    rsx! {
        p { class: "text-text mb-4", "You are already a member of this board with your current key." }
        button {
            class: "px-4 py-2 bg-accent hover:bg-accent-hover text-white font-medium rounded-lg transition-colors",
            autofocus: true,
            onmounted: move |cx| {
                let element = cx.data();
                wasm_bindgen_futures::spawn_local(async move {
                    let _ = element.set_focus(true).await;
                });
            },
            onclick: move |_| {
                navigator().push(Route::Home);
            },
            "Close"
        }
    }
}

/// Renders the UI for restoring access to an existing member
fn render_restore_access_option(inv: Invitation) -> Element {
    let board = inv.board;
    let member_vk = inv.invitee.member.member_vk;
    let invitee = inv.invitee.clone();

    rsx! {
        p { class: "text-text mb-2", "This invitation is for a member that already exists in the board." }
        p { class: "text-text-muted mb-4", "If you lost access to your previous key, you can use this invitation to restore access with your current key." }
        div {
            class: "flex gap-3",
            button {
                class: "px-4 py-2 bg-yellow-500 hover:bg-yellow-600 text-white font-medium rounded-lg transition-colors",
                autofocus: true,
                onmounted: move |cx| {
                    let element = cx.data();
                    wasm_bindgen_futures::spawn_local(async move {
                        let _ = element.set_focus(true).await;
                    });
                },
                onclick: {
                    let invitee = invitee.clone();
                    move |_| {
                        // Use with_mut for atomic update
                        BOARDS.with_mut(|boards| {
                            if let Some(board_data) = boards.map.get_mut(&board) {
                                board_data.restore_member_access(member_vk, invitee.clone());
                            }
                        });
                        // Mark board as needing sync after restoring member access
                        NEEDS_SYNC.write().insert(board);
                        let board_id = bs58::encode(board.as_bytes()).into_string();
                        navigator().push(Route::Posts { board_id });
                    }
                },
                "Restore Access"
            }
            button {
                class: "px-4 py-2 bg-surface hover:bg-surface-hover text-text rounded-lg transition-colors",
                onclick: move |_| {
                    navigator().push(Route::Home);
                },
                "Cancel"
            }
        }
    }
}

/// Renders the UI for a new invitation
fn render_new_invitation(inv: Invitation) -> Element {
    let inv_for_accept = inv.clone();
    let inv_for_enter = inv.clone();

    // Generate a default nickname from the member's key
    let encoded = bs58::encode(inv.invitee.member.member_vk.as_bytes()).into_string();
    let shortened = encoded.chars().take(6).collect::<String>();
    let default_nickname = format!("User-{}", shortened);

    // Create a signal for the nickname
    let mut nickname = use_signal(|| default_nickname);

    rsx! {
        p { class: "text-text mb-2", "You have been invited to join a new board." }
        p { class: "text-text-muted mb-4", "Choose a nickname to use in this board:" }

        div { class: "mb-4",
            input {
                class: "w-full px-3 py-2 bg-surface border border-border rounded-lg text-text placeholder-text-muted focus:outline-none focus:ring-2 focus:ring-accent focus:border-transparent",
                r#type: "text",
                value: "{nickname}",
                autofocus: true,
                oninput: move |evt| nickname.set(evt.value().clone()),
                onkeydown: move |evt: KeyboardEvent| {
                    if evt.key() == Key::Enter && !nickname.read().trim().is_empty() {
                        evt.prevent_default();
                        accept_invitation(inv_for_enter.clone(), nickname.read().clone());
                    }
                },
                placeholder: "Your preferred nickname"
            }
        }

        p { class: "text-text mb-4", "Would you like to accept the invitation?" }
        div {
            class: "flex gap-3",
            button {
                class: "px-4 py-2 bg-accent hover:bg-accent-hover text-white font-medium rounded-lg transition-colors disabled:opacity-50 disabled:cursor-not-allowed",
                disabled: nickname.read().trim().is_empty(),
                onclick: move |_| {
                    accept_invitation(inv_for_accept.clone(), nickname.read().clone());
                },
                "Accept"
            }
            button {
                class: "px-4 py-2 bg-surface hover:bg-surface-hover text-text rounded-lg transition-colors",
                onclick: move |_| {
                    navigator().push(Route::Home);
                },
                "Decline"
            }
        }
    }
}

/// Handles the invitation acceptance process
fn accept_invitation(inv: Invitation, nickname: String) {
    let board_owner = inv.board;
    let authorized_member = inv.invitee.clone();
    let invitee_signing_key = inv.invitee_signing_key.clone();

    // Use the user-provided nickname
    let nickname = if nickname.trim().is_empty() {
        // Fallback to generated nickname if somehow empty
        let encoded = bs58::encode(authorized_member.member.member_vk.as_bytes()).into_string();
        let shortened = encoded.chars().take(6).collect::<String>();
        format!("User-{}", shortened)
    } else {
        nickname
    };

    info!(
        "Adding board to pending invites: {:?}",
        MemberId::from(board_owner)
    );

    // Add to pending invites
    PENDING_INVITES.with_mut(|pending_invites| {
        pending_invites.map.insert(
            board_owner,
            PendingBoardJoin {
                authorized_member: authorized_member.clone(),
                invitee_signing_key: invitee_signing_key.clone(),
                preferred_nickname: nickname.clone(),
                status: PendingBoardStatus::PendingSubscription,
                subscribing_since: None,
            },
        );
    });

    info!("Requesting board state for invitation");

    // Send the AcceptInvitation message directly without spawn_local
    let result = SYNCHRONIZER
        .write()
        .get_message_sender()
        .unbounded_send(SynchronizerMessage::AcceptInvitation {
            owner_vk: board_owner,
            authorized_member: Box::new(authorized_member),
            invitee_signing_key: Box::new(invitee_signing_key),
            nickname,
        })
        .map_err(|e| format!("Failed to send message: {}", e));

    match result {
        Ok(_) => {
            info!("Successfully requested board state for invitation");
        }
        Err(e) => {
            // Log detailed error information
            error!("Failed to request board state for invitation: {}", e);
            error!(
                "Error details: invitation for board with owner key: {:?}",
                MemberId::from(board_owner)
            );
        }
    }
}
