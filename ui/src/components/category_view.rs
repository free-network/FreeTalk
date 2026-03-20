use crate::components::conversation::MessageData;
use dioxus::prelude::*;
use river_core::board_state::member::MemberId;
use river_core::board_state::message::MessageId;

/// A card component for displaying a category in a grid
#[component]
pub fn CategoryCard(
    /// The category data
    category: MessageData,
    /// Self member ID for determining edit/delete permissions
    self_member_id: MemberId,
    /// Click handler for navigation
    on_click: EventHandler<()>,
    /// Edit handler (category_id, new_name, new_description)
    #[props(default)]
    on_edit: Option<EventHandler<(MessageId, String, String)>>,
    /// Delete handler
    #[props(default)]
    on_request_delete: Option<EventHandler<MessageId>>,
) -> Element {
    let mut is_hovered = use_signal(|| false);
    let mut is_editing = use_signal(|| false);
    let mut edit_name = use_signal(String::new);
    let mut edit_description = use_signal(String::new);

    let icon = category.category_icon.as_deref().unwrap_or("📁");
    let name = category.category_name.as_deref().unwrap_or("Unnamed");
    let description = category.category_description.as_deref();
    let color = category.category_color.as_deref().unwrap_or("#6366f1");

    let is_self = category.author_id == self_member_id;
    let has_actions = is_self && (on_edit.is_some() || on_request_delete.is_some());
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
                style: "background-color: {color};",
            }

            div { class: "p-4",
                // Icon and name row
                div { class: "flex items-center gap-3 mb-2",
                    span { class: "text-3xl", "{icon}" }
                    if *is_editing.read() {
                        input {
                            class: "flex-1 px-2 py-1 bg-surface border border-border rounded text-text text-lg font-semibold",
                            value: "{edit_name}",
                            onclick: move |e| e.stop_propagation(),
                            oninput: move |e| edit_name.set(e.value()),
                        }
                    } else {
                        h3 { class: "text-lg font-semibold text-text group-hover:text-accent transition-colors truncate",
                            "{name}"
                        }
                    }
                }

                // Description
                if *is_editing.read() {
                    textarea {
                        class: "w-full px-2 py-1 bg-surface border border-border rounded text-sm text-text resize-none",
                        rows: 2,
                        value: "{edit_description}",
                        placeholder: "Description (optional)",
                        onclick: move |e| e.stop_propagation(),
                        oninput: move |e| edit_description.set(e.value()),
                    }
                    // Save/Cancel buttons
                    div { class: "flex gap-2 mt-2",
                        button {
                            class: "px-3 py-1 text-sm bg-accent hover:bg-accent/80 text-white rounded transition-colors",
                            onclick: {
                                let msg_id = msg_id.clone();
                                move |e| {
                                    e.stop_propagation();
                                    if let Some(ref handler) = on_edit {
                                        handler.call((msg_id.clone(), edit_name.read().clone(), edit_description.read().clone()));
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
                                move |_| {
                                    edit_name.set(name.clone());
                                    edit_description.set(desc.clone());
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
