pub mod chat_delegate;
pub mod document_title;
pub mod freenet_api;
pub mod notifications;
pub mod receive_times;
pub mod sync_info;

use super::{admin_view::AdminView, conversation::Conversation, members::MemberList, room_list::RoomList, top_bar::TopBar};
use crate::components::app::document_title::DocumentTitleUpdater;
use crate::components::app::freenet_api::freenet_synchronizer::SynchronizerMessage;
use crate::components::app::freenet_api::freenet_synchronizer::SynchronizerStatus;
use crate::components::app::freenet_api::FreenetSynchronizer;
use crate::components::members::member_info_modal::MemberInfoModal;
use crate::components::members::Invitation;
use crate::components::posts_view::{PostsView, SinglePostView};
use crate::components::room_list::create_room_modal::CreateRoomModal;
use crate::components::room_list::edit_room_modal::EditRoomModal;
use crate::components::room_list::receive_invitation_modal::ReceiveInvitationModal;
use crate::invites::PendingInvites;
use crate::room_data::{CurrentRoom, Rooms};
use dioxus::document::{Link, Stylesheet};
use dioxus::logger::tracing::{debug, error, info};
use dioxus::prelude::*;
use ed25519_dalek::VerifyingKey;
use freenet_stdlib::client_api::WebApi;
use river_core::room_state::member::MemberId;
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
    #[route("/room/:room_id")]
    Posts { room_id: String },
    #[route("/room/:room_id/post/:post_id")]
    Post { room_id: String, post_id: String },
    #[route("/room/:room_id/conversation")]
    ConversationView { room_id: String },
    #[route("/room/:room_id/admin")]
    Admin { room_id: String },
    #[route("/:..route")]
    NotFound { route: Vec<String> },
}

pub static ROOMS: GlobalSignal<Rooms> = Global::new(initial_rooms);
pub static CURRENT_ROOM: GlobalSignal<CurrentRoom> =
    Global::new(|| CurrentRoom { owner_key: None });
pub static MEMBER_INFO_MODAL: GlobalSignal<MemberInfoModalSignal> =
    Global::new(|| MemberInfoModalSignal { member: None });
pub static EDIT_ROOM_MODAL: GlobalSignal<EditRoomModalSignal> =
    Global::new(|| EditRoomModalSignal { room: None });
pub static CREATE_ROOM_MODAL: GlobalSignal<CreateRoomModalSignal> =
    Global::new(|| CreateRoomModalSignal { show: false });
pub static PENDING_INVITES: GlobalSignal<PendingInvites> = Global::new(PendingInvites::new);
pub static SYNC_STATUS: GlobalSignal<SynchronizerStatus> =
    Global::new(|| SynchronizerStatus::Connecting);
pub static SYNCHRONIZER: GlobalSignal<FreenetSynchronizer> = Global::new(FreenetSynchronizer::new);
pub static WEB_API: GlobalSignal<Option<WebApi>> = Global::new(|| None);
pub static AUTH_TOKEN: GlobalSignal<Option<String>> = Global::new(|| None);

// Tracks which rooms need to be synced due to USER actions (not network updates)
// This prevents infinite loops where network responses trigger more syncs
pub static NEEDS_SYNC: GlobalSignal<std::collections::HashSet<VerifyingKey>> =
    Global::new(std::collections::HashSet::new);

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

    #[cfg(not(feature = "no-sync"))]
    {
        // The synchronizer is now started in the auth token effect

        // Watch NEEDS_SYNC signal for USER-initiated changes only
        // This prevents infinite loops from network response updates to ROOMS
        use_effect(move || {
            let rooms_needing_sync = NEEDS_SYNC.read().clone();

            if !rooms_needing_sync.is_empty() {
                info!(
                    "User changes detected for {} rooms, triggering synchronization",
                    rooms_needing_sync.len()
                );

                // Get all the data we need upfront to avoid nested borrows
                let message_sender = SYNCHRONIZER.read().get_message_sender();
                let has_rooms = !ROOMS.read().map.is_empty();
                let has_invitations = !PENDING_INVITES.read().map.is_empty();

                if has_rooms || has_invitations {
                    info!("Sending ProcessRooms message to synchronizer, has_rooms={}, has_invitations={}", has_rooms, has_invitations);

                    if let Err(e) = message_sender.unbounded_send(SynchronizerMessage::ProcessRooms)
                    {
                        error!("Failed to send ProcessRooms message: {}", e);
                    } else {
                        info!("ProcessRooms message sent successfully");

                        // Clear the sync queue after successfully sending message
                        NEEDS_SYNC.write().clear();
                    }

                    // Also save rooms to delegate when they change
                    // Use spawn_local to avoid blocking the UI thread
                    spawn_local(async {
                        if let Err(e) = chat_delegate::save_rooms_to_delegate().await {
                            error!("Failed to save rooms to delegate: {}", e);
                        }
                    });
                } else {
                    debug!("No rooms to synchronize");
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

        // Main layout with router
        RoomList {}
        TopBar {}
        Router::<Route> {}
        MemberList {}
        EditRoomModal {}
        MemberInfoModal {}
        CreateRoomModal {}
        DocumentTitleUpdater {}
    }
}

/// Route component when no room is selected
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
    let invitation = use_memo(move || {
        Invitation::from_encoded_string(&invite_code).ok()
    });

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
fn Posts(room_id: String) -> Element {
    sync_room_from_url(&room_id);
    rsx! { PostsView {} }
}

/// Route component for single post view
#[component]
fn Post(room_id: String, post_id: String) -> Element {
    sync_room_from_url(&room_id);
    rsx! { SinglePostView { post_id: post_id } }
}

/// Route component for conversation view
#[component]
fn ConversationView(room_id: String) -> Element {
    sync_room_from_url(&room_id);
    rsx! { Conversation {} }
}

/// Route component for admin management view
#[component]
fn Admin(room_id: String) -> Element {
    sync_room_from_url(&room_id);
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

/// Sync CURRENT_ROOM from URL room_id parameter
fn sync_room_from_url(room_id: &str) {
    // Parse room_id (base58-encoded VerifyingKey)
    if let Ok(bytes) = bs58::decode(room_id).into_vec() {
        if bytes.len() == 32 {
            if let Ok(vk) = VerifyingKey::from_bytes(&bytes.try_into().unwrap()) {
                // Only update if different to avoid infinite loops
                let current = CURRENT_ROOM.read().owner_key;
                if current != Some(vk) {
                    debug!("Syncing CURRENT_ROOM from URL: {}", room_id);
                    *CURRENT_ROOM.write() = CurrentRoom { owner_key: Some(vk) };
                }
            }
        }
    }
}

#[cfg(not(feature = "example-data"))]
fn initial_rooms() -> Rooms {
    Rooms {
        map: std::collections::HashMap::new(),
        current_room_key: None,
        migrated_rooms: Vec::new(),
    }
}

#[cfg(feature = "example-data")]
fn initial_rooms() -> Rooms {
    crate::example_data::create_example_rooms()
}

pub struct EditRoomModalSignal {
    pub room: Option<VerifyingKey>,
}

pub struct CreateRoomModalSignal {
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
