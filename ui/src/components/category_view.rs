use crate::components::app::{Route, BOARDS, CURRENT_BOARD};
use crate::components::conversation::{
    get_all_messages, get_category_posts, get_subcategories, MessageCard, MessageCardVariant,
    MessageData,
};
use crate::util::message_actions::{self, ActionContext};
use crate::util::messaging::ReplyContext;
use dioxus::prelude::*;
use river_core::board_state::member::MemberId;
use river_core::board_state::message::MessageId;
use wasm_bindgen_futures::spawn_local;

/// A card component for displaying a category in a grid
#[component]
pub fn CategoryCard(
    /// The category data
    category: MessageData,
    /// Click handler for navigation
    on_click: EventHandler<()>,
) -> Element {
    let icon = category.category_icon.as_deref().unwrap_or("📁");
    let name = category.category_name.as_deref().unwrap_or("Unnamed");
    let description = category.category_description.as_deref();
    let color = category.category_color.as_deref().unwrap_or("#6366f1");

    rsx! {
        div {
            class: "bg-panel rounded-xl border border-border shadow-sm overflow-hidden hover:border-accent/50 hover:shadow-md transition-all cursor-pointer group",
            onclick: move |_| on_click.call(()),

            // Color accent bar at top
            div {
                class: "h-1",
                style: "background-color: {color};",
            }

            div { class: "p-4",
                // Icon and name row
                div { class: "flex items-center gap-3 mb-2",
                    span { class: "text-3xl", "{icon}" }
                    h3 { class: "text-lg font-semibold text-text group-hover:text-accent transition-colors truncate",
                        "{name}"
                    }
                }

                // Description
                if let Some(desc) = description {
                    p { class: "text-sm text-text-muted line-clamp-2",
                        "{desc}"
                    }
                }
            }
        }
    }
}

/// Header component for category view
#[component]
fn CategoryHeader(
    category: MessageData,
    post_count: usize,
    subcategory_count: usize,
) -> Element {
    let icon = category.category_icon.as_deref().unwrap_or("📁");
    let name = category.category_name.as_deref().unwrap_or("Unnamed");
    let description = category.category_description.as_deref();
    let color = category.category_color.as_deref().unwrap_or("#6366f1");

    rsx! {
        div { class: "bg-panel rounded-xl border border-border shadow-sm overflow-hidden mb-6",
            // Color accent bar
            div {
                class: "h-2",
                style: "background-color: {color};",
            }

            div { class: "p-6",
                // Icon and name
                div { class: "flex items-center gap-4 mb-4",
                    span { class: "text-5xl", "{icon}" }
                    div { class: "flex-1 min-w-0",
                        h1 { class: "text-2xl font-bold text-text truncate", "{name}" }
                        if let Some(desc) = description {
                            p { class: "text-text-muted mt-1", "{desc}" }
                        }
                    }
                }

                // Stats
                div { class: "flex gap-4 text-sm text-text-muted",
                    if subcategory_count > 0 {
                        span {
                            {format!("{} subcategor{}", subcategory_count, if subcategory_count == 1 { "y" } else { "ies" })}
                        }
                    }
                    span {
                        {format!("{} post{}", post_count, if post_count == 1 { "" } else { "s" })}
                    }
                }
            }
        }
    }
}

/// View component for displaying category contents (subcategories and posts)
#[component]
pub fn CategoryView(category_id: String) -> Element {
    let mut pending_delete: Signal<Option<MessageId>> = use_signal(|| None);

    // Get current board_id for navigation
    let current_board_id = use_memo(move || {
        CURRENT_BOARD
            .read()
            .owner_key
            .map(|key| bs58::encode(key.as_bytes()).into_string())
            .unwrap_or_default()
    });

    // Get category data, subcategories, and posts
    let category_data = use_memo({
        let category_id = category_id.clone();
        move || {
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

                    // Find the category itself
                    let category = all_messages
                        .iter()
                        .find(|m| m.id_string() == category_id && m.is_category)
                        .cloned();

                    // Parse category_id to MessageId
                    let category_message_id = category_id.parse::<i64>().ok().map(|hash| {
                        MessageId(freenet_scaffold::util::FastHash(hash))
                    });

                    if let (Some(cat), Some(cat_id)) = (category, category_message_id) {
                        let subcategories = get_subcategories(&all_messages, &cat_id);
                        let posts = get_category_posts(&all_messages, &cat_id);

                        return Some((
                            cat,
                            subcategories,
                            posts,
                            self_member_id,
                            max_title_size,
                            max_message_size,
                        ));
                    }
                }
            }
            None
        }
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
    let handle_edit_message =
        move |target_message_id: MessageId, new_title: String, new_text: String| {
            if let Some(ctx) = ActionContext::from_current_board() {
                spawn_local(async move {
                    message_actions::edit_message(ctx, target_message_id, new_title, new_text)
                        .await;
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

            // Category content
            div { class: "flex-1 overflow-y-auto",
                div { class: "max-w-4xl mx-auto px-4 py-6",
                    {
                        match category_data.read().as_ref() {
                            Some((category, subcategories, posts, self_member_id, max_title_size, max_message_size)) => {
                                let nav = navigator();
                                let board_id = current_board_id.read().clone();
                                let subcategories = subcategories.clone();
                                let posts = posts.clone();
                                let category = category.clone();
                                let self_member_id = *self_member_id;
                                let max_title_size = *max_title_size;
                                let max_message_size = *max_message_size;

                                // Reply context for posting in category (will be used when adding post input)
                                let _category_reply_context = ReplyContext {
                                    message_id: category.message_id.clone(),
                                    author_name: category.author_name.clone(),
                                    content_preview: category.category_name.clone().unwrap_or_default(),
                                };

                                rsx! {
                                    // Category header
                                    CategoryHeader {
                                        category: category.clone(),
                                        post_count: posts.len(),
                                        subcategory_count: subcategories.len(),
                                    }

                                    // Subcategories grid
                                    if !subcategories.is_empty() {
                                        div { class: "mb-8",
                                            h2 { class: "text-lg font-semibold text-text-muted mb-4 flex items-center gap-2",
                                                "Subcategories"
                                            }
                                            div { class: "grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4",
                                                {subcategories.iter().map({
                                                    let board_id = board_id.clone();
                                                    let nav = nav.clone();
                                                    move |subcat| {
                                                        let cat_id = subcat.id_string();
                                                        let board_id = board_id.clone();
                                                        let nav = nav.clone();
                                                        rsx! {
                                                            CategoryCard {
                                                                key: "{cat_id}",
                                                                category: subcat.clone(),
                                                                on_click: move |_| {
                                                                    nav.push(Route::Category {
                                                                        board_id: board_id.clone(),
                                                                        category_id: cat_id.clone(),
                                                                    });
                                                                },
                                                            }
                                                        }
                                                    }
                                                })}
                                            }
                                        }
                                    }

                                    // Posts in category
                                    if !posts.is_empty() {
                                        div {
                                            h2 { class: "text-lg font-semibold text-text-muted mb-4 flex items-center gap-2",
                                                "Posts"
                                            }
                                            div { class: "space-y-6",
                                                {posts.iter().map({
                                                    let handle_toggle_reaction = handle_toggle_reaction.clone();
                                                    let handle_edit_message = handle_edit_message.clone();
                                                    let board_id = board_id.clone();
                                                    let nav = nav.clone();
                                                    move |post| {
                                                        let post_id = post.id_string();
                                                        let board_id = board_id.clone();
                                                        let nav = nav.clone();
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

                                    // Empty state
                                    if posts.is_empty() && subcategories.is_empty() {
                                        div { class: "flex flex-col items-center justify-center h-32 text-text-muted",
                                            p { class: "text-lg", "No content in this category yet." }
                                            p { class: "text-sm mt-2", "Create a post to get started!" }
                                        }
                                    }
                                }
                            }
                            None => {
                                rsx! {
                                    div { class: "flex flex-col items-center justify-center h-64 text-text-muted",
                                        p { class: "text-xl", "Category not found." }
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
