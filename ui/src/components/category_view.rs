use crate::components::conversation::MessageData;
use dioxus::prelude::*;
use river_core::board_state::content::validate_icon;
use river_core::board_state::member::MemberId;
use river_core::board_state::message::MessageId;

/// Color presets for category selection
const COLOR_PRESETS: &[(&str, &str)] = &[
    ("#6366f1", "Indigo"),
    ("#8b5cf6", "Violet"),
    ("#ec4899", "Pink"),
    ("#ef4444", "Red"),
    ("#f97316", "Orange"),
    ("#eab308", "Yellow"),
    ("#22c55e", "Green"),
    ("#14b8a6", "Teal"),
    ("#0ea5e9", "Sky"),
    ("#6b7280", "Gray"),
];

/// Category edit payload for the edit handler
#[derive(Clone, Debug)]
pub struct CategoryEditData {
    pub message_id: MessageId,
    pub new_name: String,
    pub new_description: Option<String>,
    pub new_icon: Option<String>,
    pub new_color: String,
}

/// A card component for displaying a category in a grid
#[component]
pub fn CategoryCard(
    /// The category data
    category: MessageData,
    /// Self member ID for determining edit/delete permissions
    self_member_id: MemberId,
    /// Whether the current user is the board owner
    #[props(default)]
    is_owner: bool,
    /// Whether the current user is a board admin
    #[props(default)]
    is_admin: bool,
    /// Click handler for navigation
    on_click: EventHandler<()>,
    /// Edit handler with full category data
    #[props(default)]
    on_edit: Option<EventHandler<CategoryEditData>>,
    /// Delete handler
    #[props(default)]
    on_request_delete: Option<EventHandler<MessageId>>,
) -> Element {
    let mut is_hovered = use_signal(|| false);
    let mut is_editing = use_signal(|| false);
    let mut edit_name = use_signal(String::new);
    let mut edit_description = use_signal(String::new);
    let mut edit_icon = use_signal(String::new);
    let mut edit_color = use_signal(String::new);
    let mut error_msg = use_signal(|| None::<String>);

    let icon = category.category_icon.as_deref().unwrap_or("📁");
    let name = category.category_name.as_deref().unwrap_or("Unnamed");
    let description = category.category_description.as_deref();
    let color = category.category_color.as_deref().unwrap_or("#6366f1");

    // Can edit if user is author, owner, or admin
    let is_author = category.author_id == self_member_id;
    let can_edit = is_author || is_owner || is_admin;
    let has_actions = can_edit && (on_edit.is_some() || on_request_delete.is_some());
    let msg_id = category.message_id.clone();

    rsx! {
        div {
            class: "bg-panel rounded-xl border border-border shadow-sm overflow-hidden hover:border-accent/50 hover:shadow-md transition-all cursor-pointer group relative",
            onclick: move |_| {
                if !*is_editing.read() {
                    on_click.call(())
                }
            },
            onmouseenter: move |_| is_hovered.set(true),
            onmouseleave: move |_| is_hovered.set(false),

            // Color accent bar at top
            div {
                class: "h-1",
                style: if *is_editing.read() {
                    format!("background-color: {};", edit_color.read())
                } else {
                    format!("background-color: {};", color)
                },
            }

            div { class: "p-4",
                // Error message
                if let Some(err) = error_msg.read().as_ref() {
                    div { class: "mb-2 p-2 bg-red-100 dark:bg-red-900/30 border border-red-200 dark:border-red-800 rounded text-xs text-red-700 dark:text-red-400",
                        "{err}"
                    }
                }

                // Icon and name row
                div { class: "flex items-center gap-3 mb-2",
                    if *is_editing.read() {
                        // Icon input
                        input {
                            class: "w-12 px-2 py-1 bg-surface border border-border rounded text-2xl text-center",
                            value: "{edit_icon}",
                            placeholder: "📁",
                            onclick: move |e| e.stop_propagation(),
                            oninput: move |e| edit_icon.set(e.value()),
                        }
                        // Name input
                        input {
                            class: "flex-1 px-2 py-1 bg-surface border border-border rounded text-text text-lg font-semibold",
                            value: "{edit_name}",
                            onclick: move |e| e.stop_propagation(),
                            oninput: move |e| edit_name.set(e.value()),
                        }
                    } else {
                        span { class: "text-3xl", "{icon}" }
                        h3 { class: "text-lg font-semibold text-text group-hover:text-accent transition-colors truncate",
                            "{name}"
                        }
                    }
                }

                // Description and color picker when editing
                if *is_editing.read() {
                    // Description
                    textarea {
                        class: "w-full px-2 py-1 bg-surface border border-border rounded text-sm text-text resize-none mb-2",
                        rows: 2,
                        value: "{edit_description}",
                        placeholder: "Description (optional)",
                        onclick: move |e| e.stop_propagation(),
                        oninput: move |e| edit_description.set(e.value()),
                    }

                    // Color picker
                    div { class: "mb-2",
                        label { class: "block text-xs font-medium text-text-muted mb-1", "Color" }
                        div { class: "flex flex-wrap gap-1",
                            {COLOR_PRESETS.iter().map(|(hex, label)| {
                                let hex = *hex;
                                let is_selected = *edit_color.read() == hex;
                                rsx! {
                                    button {
                                        key: "{hex}",
                                        class: if is_selected {
                                            "w-6 h-6 rounded-full ring-2 ring-offset-1 ring-offset-panel ring-text relative flex items-center justify-center"
                                        } else {
                                            "w-6 h-6 rounded-full hover:scale-110 transition-transform"
                                        },
                                        style: "background-color: {hex};",
                                        title: "{label}",
                                        onclick: move |e| {
                                            e.stop_propagation();
                                            edit_color.set(hex.to_string());
                                        },
                                        if is_selected {
                                            span { class: "text-white text-xs font-bold drop-shadow-[0_1px_1px_rgba(0,0,0,0.5)]", "✓" }
                                        }
                                    }
                                }
                            })}
                        }
                    }

                    // Save/Cancel buttons
                    div { class: "flex gap-2",
                        button {
                            class: "px-3 py-1 text-sm bg-accent hover:bg-accent/80 text-white rounded transition-colors",
                            onclick: {
                                let msg_id = msg_id.clone();
                                move |e| {
                                    e.stop_propagation();

                                    // Validate icon
                                    let icon_val = edit_icon.read().trim().to_string();
                                    if !icon_val.is_empty() {
                                        if let Err(err) = validate_icon(&icon_val) {
                                            error_msg.set(Some(err));
                                            return;
                                        }
                                    }

                                    // Validate name
                                    let name_val = edit_name.read().trim().to_string();
                                    if name_val.is_empty() {
                                        error_msg.set(Some("Name cannot be empty".to_string()));
                                        return;
                                    }

                                    error_msg.set(None);

                                    if let Some(ref handler) = on_edit {
                                        let desc = edit_description.read().trim().to_string();
                                        handler.call(CategoryEditData {
                                            message_id: msg_id.clone(),
                                            new_name: name_val,
                                            new_description: if desc.is_empty() { None } else { Some(desc) },
                                            new_icon: if icon_val.is_empty() { None } else { Some(icon_val) },
                                            new_color: edit_color.read().clone(),
                                        });
                                    }
                                    is_editing.set(false);
                                }
                            },
                            "Save"
                        }
                        button {
                            class: "px-3 py-1 text-sm bg-surface hover:bg-surface-hover text-text rounded transition-colors",
                            onclick: move |e| {
                                e.stop_propagation();
                                error_msg.set(None);
                                is_editing.set(false);
                            },
                            "Cancel"
                        }
                    }
                } else if let Some(desc) = description {
                    p { class: "text-sm text-text-muted line-clamp-2",
                        "{desc}"
                    }
                }
            }

            // Action buttons overlay (on hover)
            if has_actions && *is_hovered.read() && !*is_editing.read() {
                div {
                    class: "absolute right-2 top-3 flex gap-1 bg-panel rounded shadow border border-border px-1 py-0.5",
                    onclick: move |e| e.stop_propagation(),

                    if on_edit.is_some() {
                        button {
                            class: "text-xs text-text-muted hover:text-text px-1",
                            onclick: {
                                let name = name.to_string();
                                let desc = description.map(|s| s.to_string()).unwrap_or_default();
                                let ic = icon.to_string();
                                let col = color.to_string();
                                move |_| {
                                    edit_name.set(name.clone());
                                    edit_description.set(desc.clone());
                                    edit_icon.set(ic.clone());
                                    edit_color.set(col.clone());
                                    error_msg.set(None);
                                    is_editing.set(true);
                                }
                            },
                            "edit"
                        }
                    }
                    if let Some(ref delete_handler) = on_request_delete {
                        button {
                            class: "text-xs text-text-muted hover:text-red-500 px-1",
                            onclick: {
                                let handler = delete_handler.clone();
                                let msg_id = msg_id.clone();
                                move |_| handler.call(msg_id.clone())
                            },
                            "delete"
                        }
                    }
                }
            }
        }
    }
}
