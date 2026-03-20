use crate::components::app::{Route, BOARDS, CURRENT_BOARD};
use crate::components::category_view::CategoryCard;
use crate::components::conversation::{
    get_all_messages, get_top_level_categories, get_top_level_posts, MessageCard,
    MessageCardVariant,
};
use crate::util::message_actions::{self, ActionContext};
use dioxus::prelude::*;
use river_core::board_state::member::MemberId;
use river_core::board_state::message::MessageId;
use wasm_bindgen_futures::spawn_local;

#[component]
pub fn PostsView() -> Element {
    let mut pending_delete: Signal<Option<MessageId>> = use_signal(|| None);

    let current_board_data = {
        let current_board = CURRENT_BOARD.read();
        if let Some(key) = current_board.owner_key {
            let boards = BOARDS.read();
            boards.map.get(&key).cloned()
        } else {
            None
        }
    };

    let has_board_selected = current_board_data.is_some();

    // Get top-level categories and posts (memoized)
    let content = use_memo(move || {
        let current_board = CURRENT_BOARD.read();
        if let Some(key) = current_board.owner_key {
            let boards = BOARDS.read();
            if let Some(board_data) = boards.map.get(&key) {
                let self_member_id = MemberId::from(&board_data.self_sk.verifying_key());
                let all_messages = get_all_messages(
                    &board_data.board_state.recent_messages,
                    &board_data.board_state.member_info,
                    self_member_id,
                    &board_data.secrets,
                );
                let categories = get_top_level_categories(&all_messages);
                let posts = get_top_level_posts(&all_messages);
                return Some((categories, posts));
            }
        }
        None
    });

    // Handler for toggling reactions
    let handle_toggle_reaction = move |target_message_id: MessageId, emoji: String| {
        if let Some(ctx) = ActionContext::from_current_board() {
            spawn_local(async move {
                message_actions::toggle_reaction(ctx, target_message_id, emoji).await;
            });
        }
    };

    // Handler for deleting messages
    let handle_delete_message = move |target_message_id: MessageId| {
        if let Some(ctx) = ActionContext::from_current_board() {
            spawn_local(async move {
                message_actions::delete_message(ctx, target_message_id).await;
            });
        }
    };

    // Handler for editing messages
    let handle_edit_message = move |target_message_id: MessageId,
                                    new_title: String,
                                    new_text: String| {
        if let Some(ctx) = ActionContext::from_current_board() {
            spawn_local(async move {
                message_actions::edit_message(ctx, target_message_id, new_title, new_text).await;
            });
        }
    };

    rsx! {
        div { class: "flex-1 flex flex-col min-w-0 bg-bg",
            // Show no-board-selected message or header with user info
            if !has_board_selected {
                div { class: "flex flex-col items-center justify-center h-64 text-text-muted",
                    p { class: "text-xl", "Select a board from the sidebar above or create one" }
                    p { class: "text-sm mt-2", "Posts will appear here" }
                }
            } else {
                // Categories and Posts list
                div { class: "flex-1 overflow-y-auto",
                    div { class: "max-w-4xl mx-auto px-4 py-6",
                        {
                            let board_info = current_board_data.as_ref()
                                .map(|rd| {
                                    let config = &rd.board_state.configuration.configuration;
                                    (
                                        MemberId::from(&rd.self_sk.verifying_key()),
                                        config.max_title_size,
                                        config.max_message_size,
                                    )
                                });
                            match (content.read().as_ref(), board_info) {
                                (Some((categories, posts)), Some((self_member_id, max_title_size, max_message_size))) => {
                                    let has_categories = !categories.is_empty();
                                    let has_posts = !posts.is_empty();

                                    if !has_categories && !has_posts {
                                        rsx! {
                                            div { class: "flex flex-col items-center justify-center h-64 text-text-muted",
                                                p { class: "text-xl", "No posts yet." }
                                                p { class: "text-sm mt-2", "Be the first to share something!" }
                                            }
                                        }
                                    } else {
                                        rsx! {
                                            // Categories grid
                                            if has_categories {
                                                div { class: "mb-8",
                                                    h2 { class: "text-lg font-semibold text-text-muted mb-4 flex items-center gap-2",
                                                        "Categories"
                                                    }
                                                    div { class: "grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4",
                                                        {categories.iter().map(|cat| {
                                                            let cat_id = cat.id_string();
                                                            let nav = navigator();
                                                            rsx! {
                                                                CategoryCard {
                                                                    key: "{cat_id}",
                                                                    category: cat.clone(),
                                                                    on_click: move |_| {
                                                                        if let Some(key) = CURRENT_BOARD.read().owner_key {
                                                                            let board_id = bs58::encode(key.as_bytes()).into_string();
                                                                            nav.push(Route::Category {
                                                                                board_id,
                                                                                category_id: cat_id.clone(),
                                                                            });
                                                                        }
                                                                    },
                                                                }
                                                            }
                                                        })}
                                                    }
                                                }
                                            }

                                            // Posts section
                                            if has_posts {
                                                if has_categories {
                                                    h2 { class: "text-lg font-semibold text-text-muted mb-4 flex items-center gap-2",
                                                        "Posts"
                                                    }
                                                }
                                                div { class: "space-y-8",
                                                    {posts.iter().map({
                                                        let handle_toggle_reaction = handle_toggle_reaction.clone();
                                                        let handle_edit_message = handle_edit_message.clone();
                                                        move |post| {
                                                            let post_id = post.id_string();
                                                            let nav = navigator();
                                                            let handle_toggle_reaction = handle_toggle_reaction.clone();
                                                            let handle_edit_message = handle_edit_message.clone();
                                                            rsx! {
                                                                div { key: "{post_id}",
                                                                    MessageCard {
                                                                        message: post.clone(),
                                                                        variant: MessageCardVariant::Card,
                                                                        self_member_id: self_member_id,
                                                                        expanded: false,
                                                                        show_replies: false,
                                                                        max_title_size: max_title_size,
                                                                        max_message_size: max_message_size,
                                                                        on_click: move |_| {
                                                                            // Get current board_id for navigation
                                                                            if let Some(key) = CURRENT_BOARD.read().owner_key {
                                                                                let board_id = bs58::encode(key.as_bytes()).into_string();
                                                                                nav.push(Route::Post { board_id, post_id: post_id.clone() });
                                                                            }
                                                                        },
                                                                        on_react: move |(msg_id, emoji)| {
                                                                            handle_toggle_reaction(msg_id, emoji);
                                                                        },
                                                                        on_request_delete: move |msg_id| {
                                                                            pending_delete.set(Some(msg_id));
                                                                        },
                                                                        on_edit: move |(msg_id, new_title, new_text)| {
                                                                            handle_edit_message(msg_id, new_title, new_text);
                                                                        },
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    })}
                                                }
                                            }
                                        }
                                    }
                                }
                                _ => {
                                    rsx! {
                                        div { class: "flex flex-col items-center justify-center h-64 text-text-muted",
                                            p { class: "text-xl", "No posts yet." }
                                            p { class: "text-sm mt-2", "Be the first to share something!" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Delete confirmation modal
                if pending_delete.read().is_some() {
                    div {
                        class: "fixed inset-0 bg-black/50 flex items-center justify-center z-50",
                        onclick: move |_| pending_delete.set(None),
                        div {
                            class: "bg-panel rounded-lg shadow-xl p-6 max-w-sm mx-4",
                            onclick: move |e| e.stop_propagation(),
                            h3 { class: "text-lg font-semibold text-text mb-2",
                                "Delete Post?"
                            }
                            p { class: "text-text-muted text-sm mb-4",
                                "This action cannot be undone."
                            }
                            div { class: "flex gap-3 justify-end",
                                button {
                                    class: "px-4 py-2 rounded-lg bg-surface hover:bg-surface/80 text-text transition-colors",
                                    onclick: move |_| pending_delete.set(None),
                                    "Cancel"
                                }
                                button {
                                    class: "px-4 py-2 rounded-lg bg-red-500 hover:bg-red-600 text-white transition-colors",
                                    onclick: move |_| {
                                        let msg_id_opt = pending_delete.read().clone();
                                        if let Some(msg_id) = msg_id_opt {
                                            handle_delete_message(msg_id);
                                        }
                                        pending_delete.set(None);
                                    },
                                    "Delete"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Single post view - displays a single post by ID with replies
#[component]
pub fn SinglePostView(post_id: String) -> Element {
    let mut pending_delete: Signal<Option<MessageId>> = use_signal(|| None);

    // Get current board_id for navigation
    let current_board_id = use_memo(move || {
        CURRENT_BOARD
            .read()
            .owner_key
            .map(|key| bs58::encode(key.as_bytes()).into_string())
            .unwrap_or_default()
    });

    // Find the post, self_member_id, and config
    let post_data = use_memo(move || {
        let current_board = CURRENT_BOARD.read();
        if let Some(key) = current_board.owner_key {
            let boards = BOARDS.read();
            if let Some(board_data) = boards.map.get(&key) {
                let self_member_id = MemberId::from(&board_data.self_sk.verifying_key());
                let config = &board_data.board_state.configuration.configuration;
                let max_title_size = config.max_title_size;
                let max_message_size = config.max_message_size;
                let all_messages = get_all_messages(
                    &board_data.board_state.recent_messages,
                    &board_data.board_state.member_info,
                    self_member_id,
                    &board_data.secrets,
                );
                let post = all_messages.into_iter().find(|m| m.id_string() == post_id);
                return post.map(|p| (p, self_member_id, max_title_size, max_message_size));
            }
        }
        None
    });

    // Handler for toggling reactions
    let handle_toggle_reaction = move |target_message_id: MessageId, emoji: String| {
        if let Some(ctx) = ActionContext::from_current_board() {
            spawn_local(async move {
                message_actions::toggle_reaction(ctx, target_message_id, emoji).await;
            });
        }
    };

    // Handler for deleting messages
    let handle_delete_message = move |target_message_id: MessageId| {
        if let Some(ctx) = ActionContext::from_current_board() {
            spawn_local(async move {
                message_actions::delete_message(ctx, target_message_id).await;
            });
        }
    };

    // Handler for editing messages
    let handle_edit_message = move |target_message_id: MessageId,
                                    new_title: String,
                                    new_text: String| {
        if let Some(ctx) = ActionContext::from_current_board() {
            spawn_local(async move {
                message_actions::edit_message(ctx, target_message_id, new_title, new_text).await;
            });
        }
    };

    rsx! {
        div { class: "flex-1 flex flex-col min-w-0 bg-bg",
            // Back button header
            div { class: "flex items-center gap-4 px-6 py-4 border-b border-border",
                Link {
                    to: Route::Posts { board_id: current_board_id.read().clone() },
                    class: "text-accent hover:text-accent/80 transition-colors text-lg",
                    "← Back to posts"
                }
            }

            // Post content and replies
            div { class: "flex-1 overflow-y-auto",
                {
                    match post_data.read().as_ref() {
                        Some((post, self_member_id, max_title_size, max_message_size)) => {
                            rsx! {
                                div { class: "max-w-4xl mx-auto px-4 py-6",
                                    MessageCard {
                                        message: post.clone(),
                                        variant: MessageCardVariant::Card,
                                        self_member_id: *self_member_id,
                                        expanded: true,
                                        show_replies: true,
                                        max_title_size: *max_title_size,
                                        max_message_size: *max_message_size,
                                        on_react: move |(msg_id, emoji)| {
                                            handle_toggle_reaction(msg_id, emoji);
                                        },
                                        on_request_delete: move |msg_id| {
                                            pending_delete.set(Some(msg_id));
                                        },
                                        on_edit: move |(msg_id, new_title, new_text)| {
                                            handle_edit_message(msg_id, new_title, new_text);
                                        },
                                    }
                                }
                            }
                        }
                        None => {
                            rsx! {
                                div { class: "flex-1 flex flex-col items-center justify-center h-64 text-text-muted",
                                    p { class: "text-xl", "Post not found." }
                                    Link {
                                        to: Route::Posts { board_id: current_board_id.read().clone() },
                                        class: "mt-4 text-accent hover:text-accent/80 transition-colors",
                                        "← Back to posts"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Delete confirmation modal
            if pending_delete.read().is_some() {
                div {
                    class: "fixed inset-0 bg-black/50 flex items-center justify-center z-50",
                    onclick: move |_| pending_delete.set(None),
                    div {
                        class: "bg-panel rounded-lg shadow-xl p-6 max-w-sm mx-4",
                        onclick: move |e| e.stop_propagation(),
                        h3 { class: "text-lg font-semibold text-text mb-2",
                            "Delete Post?"
                        }
                        p { class: "text-text-muted text-sm mb-4",
                            "This action cannot be undone."
                        }
                        div { class: "flex gap-3 justify-end",
                            button {
                                class: "px-4 py-2 rounded-lg bg-surface hover:bg-surface/80 text-text transition-colors",
                                onclick: move |_| pending_delete.set(None),
                                "Cancel"
                            }
                            button {
                                class: "px-4 py-2 rounded-lg bg-red-500 hover:bg-red-600 text-white transition-colors",
                                onclick: move |_| {
                                    let msg_id_opt = pending_delete.read().clone();
                                    if let Some(msg_id) = msg_id_opt {
                                        handle_delete_message(msg_id);
                                    }
                                    pending_delete.set(None);
                                },
                                "Delete"
                            }
                        }
                    }
                }
            }
        }
    }
}
