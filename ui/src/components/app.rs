pub mod chat_delegate;
pub mod document_title;
pub mod freenet_api;
pub mod notifications;
pub mod receive_times;
pub mod sync_info;

use super::{
    admin_view::AdminView, board_list::BoardList, conversation::Conversation, members::MemberList,
    top_bar::TopBar,
};
use crate::board_data::{Boards, CurrentBoard};
use crate::components::app::document_title::DocumentTitleUpdater;
use crate::components::app::freenet_api::freenet_synchronizer::SynchronizerMessage;
use crate::components::app::freenet_api::freenet_synchronizer::SynchronizerStatus;
use crate::components::app::freenet_api::FreenetSynchronizer;
use crate::components::board_list::create_board_modal::CreateBoardModal;
use crate::components::board_list::edit_board_modal::EditBoardModal;
use crate::components::board_list::receive_invitation_modal::ReceiveInvitationModal;
use crate::components::members::member_info_modal::MemberInfoModal;
use crate::components::members::Invitation;
use crate::components::posts_view::{PostsView, SinglePostView};
use crate::invites::PendingInvites;
use dioxus::document::{Link, Stylesheet};
use dioxus::logger::tracing::{debug, error, info};
use dioxus::prelude::*;
use ed25519_dalek::VerifyingKey;
use freenet_stdlib::client_api::WebApi;
use river_core::board_state::member::MemberId;
use wasm_bindgen_futures::spawn_local;
use web_sys::window;

/// Application routes
#[derive(Clone, Debug, PartialEq, Routable)]
#[rustfmt::skip]
pub enum Route {
    #[route("/")]
    Home,
    #[route("/invite/:invite_code")]
    Invite { invite_code: String },
    #[route("/board/:board_id")]
    Posts { board_id: String },
    #[route("/board/:board_id/post/:post_id")]
    Post { board_id: String, post_id: String },
    #[route("/board/:board_id/conversation")]
    ConversationView { board_id: String },
    #[route("/board/:board_id/admin")]
    Admin { board_id: String },
    #[route("/:..route")]
    NotFound { route: Vec<String> },
}

pub static BOARDS: GlobalSignal<Boards> = Global::new(initial_boards);
pub static CURRENT_BOARD: GlobalSignal<CurrentBoard> =
    Global::new(|| CurrentBoard { owner_key: None });
pub static MEMBER_INFO_MODAL: GlobalSignal<MemberInfoModalSignal> =
    Global::new(|| MemberInfoModalSignal { member: None });
pub static EDIT_BOARD_MODAL: GlobalSignal<EditBoardModalSignal> =
    Global::new(|| EditBoardModalSignal { board: None });
pub static CREATE_BOARD_MODAL: GlobalSignal<CreateBoardModalSignal> =
    Global::new(|| CreateBoardModalSignal { show: false });
pub static PENDING_INVITES: GlobalSignal<PendingInvites> = Global::new(PendingInvites::new);
pub static SYNC_STATUS: GlobalSignal<SynchronizerStatus> =
    Global::new(|| SynchronizerStatus::Connecting);
pub static SYNCHRONIZER: GlobalSignal<FreenetSynchronizer> = Global::new(FreenetSynchronizer::new);
pub static WEB_API: GlobalSignal<Option<WebApi>> = Global::new(|| None);
pub static AUTH_TOKEN: GlobalSignal<Option<String>> = Global::new(|| None);

// Tracks which boards need to be synced due to USER actions (not network updates)
// This prevents infinite loops where network responses trigger more syncs
pub static NEEDS_SYNC: GlobalSignal<std::collections::HashSet<VerifyingKey>> =
    Global::new(std::collections::HashSet::new);

/// Mark a board as needing sync, deferred via setTimeout(0).
///
/// IMPORTANT: Writing to NEEDS_SYNC triggers a Dioxus use_effect synchronously,
/// which cascades into ProcessBoards → BOARDS.read() and other signal reads.
/// If called while any signal is borrowed, this causes a RefCell re-entrant
/// borrow panic in WASM (especially on Firefox mobile).
///
/// We use setTimeout(0) instead of spawn_local because spawn_local runs within
/// wasm-bindgen-futures' task scheduler, which may itself hold a RefCell borrow
/// when polling tasks. setTimeout(0) breaks out of the WASM call stack entirely,
/// ensuring the write happens in a completely clean execution context.
pub fn mark_needs_sync(board_key: ed25519_dalek::VerifyingKey) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::prelude::*;
        let cb = Closure::once_into_js(move || {
            NEEDS_SYNC.write().insert(board_key);
        });
        web_sys::window()
            .expect("no window")
            .set_timeout_with_callback(&cb.into())
            .ok();
    }
    #[cfg(not(target_arch = "wasm32"))]
    NEEDS_SYNC.write().insert(board_key);
}

// Build metadata from build.rs
const BUILD_TIMESTAMP: &str = env!("BUILD_TIMESTAMP_ISO");
const GIT_COMMIT: &str = env!("GIT_COMMIT_HASH");

#[component]
pub fn App() -> Element {
    info!(
        "FreeTalk UI loaded - Built: {} | Commit: {}",
        BUILD_TIMESTAMP, GIT_COMMIT
    );

    // Get auth token from window global (injected by Freenet gateway)
    // This is synchronous - no network request needed
    get_auth_token_from_window();

    // Start synchronizer - auth token is already available
    spawn_local(async {
        debug!("Starting FreenetSynchronizer from App component");
        // Note: The synchronizer will set up the chat delegate after connection is established
        let mut synchronizer = SYNCHRONIZER.write();
        synchronizer.start().await;
    });

    // Watch CURRENT_BOARD and navigate if URL doesn't match
    // This handles the case where sync/delegate sets the current board but URL remains unchanged
    use_effect(move || {
        let current_board = CURRENT_BOARD.read();
        if let Some(owner_key) = current_board.owner_key {
            let board_id = bs58::encode(owner_key.as_bytes()).into_string();

            // Check if URL already shows this board to avoid race conditions with regular navigation
            let url_matches = if let Some(window) = window() {
                if let Ok(hash) = window.location().hash() {
                    // URL hash is like "#/board/{board_id}" or "#/board/{board_id}/..."
                    hash.contains(&format!("/board/{}", board_id))
                } else {
                    false
                }
            } else {
                false
            };

            if !url_matches {
                debug!(
                    "CURRENT_BOARD changed to {} but URL doesn't match, navigating",
                    board_id
                );
                if let Some(window) = window() {
                    if let Err(e) = window.location().set_hash(&format!("#/board/{}", board_id)) {
                        error!("Failed to navigate to board: {:?}", e);
                    }
                }
            }
        }
    });

    #[cfg(not(feature = "no-sync"))]
    {
        // The synchronizer is now started in the auth token effect

        // Watch NEEDS_SYNC signal for USER-initiated changes only
        // This prevents infinite loops from network response updates to boards
        use_effect(move || {
            let boards_needing_sync = NEEDS_SYNC.read().clone();

            if !boards_needing_sync.is_empty() {
                info!(
                    "User changes detected for {} boards, triggering synchronization",
                    boards_needing_sync.len()
                );

                // Get all the data we need upfront to avoid nested borrows
                let message_sender = SYNCHRONIZER.read().get_message_sender();
                let has_boards = !BOARDS.read().map.is_empty();
                let has_invitations = !PENDING_INVITES.read().map.is_empty();

                if has_boards || has_invitations {
                    info!("Sending ProcessBoards message to synchronizer, has_boards={}, has_invitations={}", has_boards, has_invitations);

                    if let Err(e) =
                        message_sender.unbounded_send(SynchronizerMessage::ProcessBoards)
                    {
                        error!("Failed to send ProcessBoards message: {}", e);
                    } else {
                        info!("ProcessBoards message sent successfully");

                        // Clear the sync queue after successfully sending message
                        NEEDS_SYNC.write().clear();
                    }

                    // Also save boards to delegate when they change
                    // Use spawn_local to avoid blocking the UI thread
                    spawn_local(async {
                        if let Err(e) = chat_delegate::save_boards_to_delegate().await {
                            error!("Failed to save boards to delegate: {}", e);
                        }
                    });
                } else {
                    debug!("No boards to synchronize");
                    // Clear the queue even if there's nothing to sync
                    NEEDS_SYNC.write().clear();
                }
            }
        });

        info!("FreenetSynchronizer setup complete");
    }

    rsx! {
        // Favicon
        Link { rel: "icon", r#type: "image/svg+xml", href: asset!("/assets/river_logo.svg") }
        // Stylesheets
        Stylesheet { href: asset!("/assets/styles.css") }
        Stylesheet { href: asset!("/assets/main.css") }

        // Main layout with router - app-root fixes iOS Safari viewport issues
        div { class: "flex bg-bg overflow-hidden app-root",
            BoardList {}
            TopBar {}
            Router::<Route> {}
            MemberList {}
            EditBoardModal {}
            MemberInfoModal {}
            CreateBoardModal {}
            DocumentTitleUpdater {}
        }
    }
}

/// Route component when no board is selected
#[component]
fn Home() -> Element {
    rsx! {
        div { class: "flex-1 flex flex-col items-center justify-center h-64 text-text-muted",
            p { class: "text-xl", "Select a board from the sidebar above or create one" }
            p { class: "text-sm mt-2", "Posts will appear here" }
        }
    }
}

/// Route component for handling invitations
#[component]
fn Invite(invite_code: String) -> Element {
    let invitation = use_memo(move || Invitation::from_encoded_string(&invite_code).ok());

    match invitation() {
        Some(inv) => rsx! {
            ReceiveInvitationModal { invitation: inv }
        },
        None => rsx! {
            div { class: "flex-1 flex flex-col items-center justify-center p-8 text-center",
                h1 { class: "text-2xl font-bold text-text mb-4", "Invalid Invitation" }
                p { class: "text-text-muted mb-6", "The invitation link appears to be invalid or corrupted." }
                a {
                    href: "#/",
                    class: "px-4 py-2 bg-accent hover:bg-accent-hover text-white rounded-lg transition-colors inline-block",
                    "Go to Home"
                }
            }
        },
    }
}

/// Route component for posts list view
#[component]
fn Posts(board_id: String) -> Element {
    sync_board_from_url(&board_id);
    rsx! { PostsView {} }
}

/// Route component for single post view
#[component]
fn Post(board_id: String, post_id: String) -> Element {
    sync_board_from_url(&board_id);
    rsx! { SinglePostView { post_id: post_id } }
}

/// Route component for conversation view
#[component]
fn ConversationView(board_id: String) -> Element {
    sync_board_from_url(&board_id);
    rsx! { Conversation {} }
}

/// Route component for admin management view
#[component]
fn Admin(board_id: String) -> Element {
    sync_board_from_url(&board_id);
    rsx! { AdminView {} }
}

/// Route component for 404 not found
#[component]
fn NotFound(route: Vec<String>) -> Element {
    let path = format!("/{}", route.join("/"));
    rsx! {
        div { class: "flex-1 flex flex-col items-center justify-center p-8 text-center",
            h1 { class: "text-6xl font-bold text-text-muted mb-4", "404" }
            p { class: "text-xl text-text mb-2", "Page not found" }
            p { class: "text-text-muted mb-6", "The path \"{path}\" does not exist." }
            a {
                href: "#/",
                class: "px-4 py-2 bg-accent hover:bg-accent-hover text-white rounded-lg transition-colors inline-block",
                "Go to Home"
            }
        }
    }
}

/// Sync CURRENT_BOARD from URL board_id parameter
fn sync_board_from_url(board_id: &str) {
    // Parse board_id (base58-encoded VerifyingKey)
    if let Ok(bytes) = bs58::decode(board_id).into_vec() {
        if bytes.len() == 32 {
            if let Ok(vk) = VerifyingKey::from_bytes(&bytes.try_into().unwrap()) {
                // Only update if different to avoid infinite loops
                let current = CURRENT_BOARD.read().owner_key;
                if current != Some(vk) {
                    debug!("Syncing CURRENT_BOARD from URL: {}", board_id);
                    *CURRENT_BOARD.write() = CurrentBoard {
                        owner_key: Some(vk),
                    };
                }
            }
        }
    }
}

#[cfg(not(feature = "example-data"))]
fn initial_boards() -> Boards {
    Boards {
        map: std::collections::HashMap::new(),
        current_board_key: None,
        migrated_boards: Vec::new(),
    }
}

#[cfg(feature = "example-data")]
fn initial_boards() -> Boards {
    crate::example_data::create_example_boards()
}

pub struct EditBoardModalSignal {
    pub board: Option<VerifyingKey>,
}

pub struct CreateBoardModalSignal {
    pub show: bool,
}

pub struct MemberInfoModalSignal {
    pub member: Option<MemberId>,
}

/// Gets the authorization token from the window global variable.
/// The Freenet HTTP gateway injects this token into the HTML as:
/// <script>window.__FREENET_AUTH_TOKEN__ = "token_value";</script>
fn get_auth_token_from_window() {
    if let Some(win) = window() {
        match js_sys::Reflect::get(&win, &"__FREENET_AUTH_TOKEN__".into()) {
            Ok(token_value) => {
                if let Some(token) = token_value.as_string() {
                    info!("Found auth token from window global");
                    *AUTH_TOKEN.write() = Some(token);
                } else if token_value.is_undefined() || token_value.is_null() {
                    debug!("Auth token not injected by gateway (running locally?)");
                } else {
                    debug!("Auth token has unexpected type");
                }
            }
            Err(err) => {
                error!("Failed to read auth token from window: {:?}", err);
            }
        }
    }
}
