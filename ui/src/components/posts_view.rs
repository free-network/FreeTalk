use crate::components::app::{CurrentCategoryContext, Route, BOARDS, CURRENT_BOARD, CURRENT_CATEGORY};
use crate::components::category_view::{CategoryCard, CategoryEditData};
use crate::components::conversation::{
    get_all_messages, get_category_posts, get_subcategories, get_top_level_categories,
    get_top_level_posts, MessageCard, MessageCardVariant, MessageData,
};
use crate::util::message_actions::{self, ActionContext};
use dioxus::prelude::*;
use river_core::board_state::member::MemberId;
use river_core::board_state::message::MessageId;
use wasm_bindgen_futures::spawn_local;

/// Breadcrumb item for category navigation
#[derive(Clone, PartialEq)]
struct BreadcrumbItem {
    id: Option<String>, // None = root
    name: String,
    icon: Option<String>,
}

/// Build breadcrumb trail from root to current category
fn build_breadcrumbs(category_id: Option<&String>, all_messages: &[MessageData]) -> Vec<BreadcrumbItem> {
    let mut crumbs = vec![BreadcrumbItem {
        id: None,
        name: "Home".to_string(),
        icon: Some("🏠".to_string()),
    }];

    if let Some(cat_id) = category_id {
        // Build chain from current category up to root
        let mut chain = Vec::new();
        let mut current_id = Some(cat_id.clone());

        while let Some(cid) = current_id {
            if let Some(cat) = all_messages.iter().find(|m| m.id_string() == cid && m.is_category) {
                chain.push(BreadcrumbItem {
                    id: Some(cat.id_string()),
                    name: cat.category_name.clone().unwrap_or_else(|| "Unnamed".to_string()),
                    icon: cat.category_icon.clone(),
                });
                current_id = cat.parent_category_id.as_ref().map(|id| format!("{}", id.0.0));
            } else {
                break;
            }
        }

        // Reverse to get root-to-current order
        chain.reverse();
        crumbs.extend(chain);
    }

    crumbs
}

/// Get content (subcategories and posts) for a category or root
fn get_content(category_id: Option<&String>, all_messages: &[MessageData]) -> (Vec<MessageData>, Vec<MessageData>) {
    if let Some(cat_id_str) = category_id {
        // Parse category_id to MessageId
        if let Some(cat_msg_id) = cat_id_str.parse::<i64>().ok().map(|hash| {
            MessageId(freenet_scaffold::util::FastHash(hash))
        }) {
            let categories = get_subcategories(all_messages, &cat_msg_id);
            let posts = get_category_posts(all_messages, &cat_msg_id);
            return (categories, posts);
        }
    }
    // Root level
    let categories = get_top_level_categories(all_messages);
    let posts = get_top_level_posts(all_messages);
    (categories, posts)
}

#[component]
pub fn PostsView(
    #[props(default)] category_id: Option<String>,
) -> Element {
    let mut pending_delete: Signal<Option<MessageId>> = use_signal(|| None);

    // Get current board data
    let board_data_memo = use_memo(move || {
        let current_board = CURRENT_BOARD.read();
        if let Some(key) = current_board.owner_key {
            let boards = BOARDS.read();
            boards.map.get(&key).cloned()
        } else {
            None
        }
    });

    let current_board_data = board_data_memo.read().clone();
    let has_board_selected = current_board_data.is_some();

    // Get all messages
    let all_messages: Vec<MessageData> = if let Some(ref board_data) = current_board_data {
        let self_member_id = MemberId::from(&board_data.self_sk.verifying_key());
        get_all_messages(
            &board_data.board_state.recent_messages,
            &board_data.board_state.member_info,
            self_member_id,
            &board_data.secrets,
        )
    } else {
        Vec::new()
    };

    // Get current category if viewing one
    let current_category: Option<MessageData> = if let Some(ref cat_id) = category_id {
        all_messages.iter()
            .find(|m| m.id_string() == *cat_id && m.is_category)
            .cloned()
    } else {
        None
    };

    // Build breadcrumbs
    let breadcrumbs = build_breadcrumbs(category_id.as_ref(), &all_messages);

    // Get content
    let (categories, posts) = get_content(category_id.as_ref(), &all_messages);

    // Update CURRENT_CATEGORY for auto-parenting posts/categories
    // Compare before writing to avoid unnecessary updates
    {
        let new_context = if let Some(cat) = current_category.as_ref() {
            CurrentCategoryContext {
                category_id: Some(cat.message_id.clone()),
                category_name: cat.category_name.clone(),
                author_name: Some(cat.author_name.clone()),
            }
        } else {
            CurrentCategoryContext::default()
        };

        let current = CURRENT_CATEGORY.read();
        if current.category_id != new_context.category_id {
            drop(current);
            *CURRENT_CATEGORY.write() = new_context;
        }
    }

    // Clear CURRENT_CATEGORY when leaving
    use_drop(|| {
        *CURRENT_CATEGORY.write() = CurrentCategoryContext::default();
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

    // Handler for editing categories
    let handle_edit_category = move |edit_data: CategoryEditData| {
        if let Some(ctx) = ActionContext::from_current_board() {
            spawn_local(async move {
                message_actions::edit_category(
                    ctx,
                    edit_data.message_id,
                    edit_data.new_name,
                    edit_data.new_description,
                    edit_data.new_icon,
                    edit_data.new_color,
                )
                .await;
            });
        }
    };

    // Get current board_id for navigation
    let current_board_id = use_memo(move || {
        CURRENT_BOARD
            .read()
            .owner_key
            .map(|key| bs58::encode(key.as_bytes()).into_string())
            .unwrap_or_default()
    });

    let viewing_category = category_id.is_some();

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
                        // Breadcrumb navigation (only show when viewing a category)
                        if viewing_category {
                            div { class: "flex items-center gap-2 mb-6 text-sm flex-wrap",
                                {breadcrumbs.iter().enumerate().map(|(i, crumb)| {
                                    let is_last = i == breadcrumbs.len() - 1;
                                    let board_id = current_board_id.read().clone();
                                    let crumb_id = crumb.id.clone();
                                    let crumb_name = crumb.name.clone();
                                    let crumb_icon = crumb.icon.clone();

                                    rsx! {
                                        if i > 0 {
                                            span { class: "text-text-muted", "/" }
                                        }
                                        if is_last {
                                            span { class: "flex items-center gap-1 text-text font-medium",
                                                if let Some(icon) = crumb_icon {
                                                    span { "{icon}" }
                                                }
                                                "{crumb_name}"
                                            }
                                        } else {
                                            Link {
                                                to: if crumb_id.is_none() {
                                                    Route::Posts { board_id: board_id.clone() }
                                                } else {
                                                    Route::PostsInCategory {
                                                        board_id: board_id.clone(),
                                                        category_id: crumb_id.clone().unwrap_or_default(),
                                                    }
                                                },
                                                class: "flex items-center gap-1 text-accent hover:text-accent/80 transition-colors",
                                                if let Some(icon) = crumb_icon {
                                                    span { "{icon}" }
                                                }
                                                "{crumb_name}"
                                            }
                                        }
                                    }
                                })}
                            }
                        }

                        // Category header (when viewing a category)
                        if let Some(cat) = current_category.as_ref() {
                            {
                                let icon = cat.category_icon.as_deref().unwrap_or("📁");
                                let name = cat.category_name.as_deref().unwrap_or("Unnamed");
                                let description = cat.category_description.as_deref();
                                let color = cat.category_color.as_deref().unwrap_or("#6366f1");

                                rsx! {
                                    div { class: "bg-panel rounded-xl border border-border shadow-sm overflow-hidden mb-6",
                                        // Color accent bar
                                        div {
                                            class: "h-2",
                                            style: "background-color: {color};",
                                        }
                                        div { class: "p-6",
                                            div { class: "flex items-center gap-4",
                                                span { class: "text-4xl", "{icon}" }
                                                div { class: "flex-1 min-w-0",
                                                    h1 { class: "text-2xl font-bold text-text truncate", "{name}" }
                                                    if let Some(desc) = description {
                                                        p { class: "text-text-muted mt-1", "{desc}" }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Content
                        {
                            let board_info = current_board_data.as_ref()
                                .map(|bd| {
                                    let config = &bd.board_state.configuration.configuration;
                                    (
                                        MemberId::from(&bd.self_sk.verifying_key()),
                                        config.max_title_size,
                                        config.max_message_size,
                                    )
                                });

                            let has_categories = !categories.is_empty();
                            let has_posts = !posts.is_empty();

                            if let Some((self_member_id, max_title_size, max_message_size)) = board_info {
                                if !has_categories && !has_posts {
                                    rsx! {
                                        div { class: "flex flex-col items-center justify-center h-64 text-text-muted",
                                            p { class: "text-xl", "No content yet." }
                                            p { class: "text-sm mt-2", "Be the first to share something!" }
                                        }
                                    }
                                } else {
                                    rsx! {
                                        // Categories grid
                                        if has_categories {
                                            div { class: "mb-8",
                                                if has_posts || !viewing_category {
                                                    h2 { class: "text-lg font-semibold text-text-muted mb-4 flex items-center gap-2",
                                                        if viewing_category { "Subcategories" } else { "Categories" }
                                                    }
                                                }
                                                div { class: "grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4",
                                                    {categories.iter().map({
                                                        let handle_edit_category = handle_edit_category.clone();
                                                        move |cat| {
                                                            let cat_id = cat.id_string();
                                                            let board_id = current_board_id.read().clone();
                                                            let nav = navigator();
                                                            let handle_edit_category = handle_edit_category.clone();
                                                            rsx! {
                                                                CategoryCard {
                                                                    key: "{cat_id}",
                                                                    category: cat.clone(),
                                                                    self_member_id: self_member_id,
                                                                    on_click: move |_| {
                                                                        nav.push(Route::PostsInCategory {
                                                                            board_id: board_id.clone(),
                                                                            category_id: cat_id.clone(),
                                                                        });
                                                                    },
                                                                    on_edit: move |edit_data| {
                                                                        handle_edit_category(edit_data);
                                                                    },
                                                                    on_request_delete: move |msg_id| {
                                                                        pending_delete.set(Some(msg_id));
                                                                    },
                                                                }
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
                                                    let board_id = current_board_id.read().clone();
                                                    move |post| {
                                                        let post_id = post.id_string();
                                                        let board_id = board_id.clone();
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
                                                                        nav.push(Route::Post {
                                                                            board_id: board_id.clone(),
                                                                            post_id: post_id.clone(),
                                                                        });
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
                            } else {
                                rsx! {
                                    div { class: "flex flex-col items-center justify-center h-64 text-text-muted",
                                        p { class: "text-xl", "No content yet." }
                                        p { class: "text-sm mt-2", "Be the first to share something!" }
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
