use crate::components::app::{BOARDS, CURRENT_BOARD, CURRENT_CATEGORY};
use crate::util::messaging::send_category;
use dioxus::prelude::*;
use river_core::board_state::content::validate_icon;
use river_core::board_state::message::MessageId;
use wasm_bindgen_futures::spawn_local;

/// Global signal for category creation modal
pub static CREATE_CATEGORY_MODAL: GlobalSignal<CreateCategoryModalSignal> =
    Global::new(|| CreateCategoryModalSignal {
        show: false,
        parent_category_id: None,
    });

pub struct CreateCategoryModalSignal {
    pub show: bool,
    pub parent_category_id: Option<MessageId>,
}

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

#[component]
pub fn CategoryCreateModal() -> Element {
    let mut name = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut icon = use_signal(String::new);
    let mut color = use_signal(|| COLOR_PRESETS[0].0.to_string());
    let mut error_msg = use_signal(|| None::<String>);
    let mut is_creating = use_signal(|| false);

    let modal_state = CREATE_CATEGORY_MODAL.read();
    let is_open = modal_state.show;
    let parent_id = modal_state.parent_category_id.clone();
    let parent_id_for_closure = parent_id.clone();
    let has_parent = parent_id.is_some();

    if !is_open {
        return rsx! {};
    }

    // Check if user can create categories
    let can_create = {
        let current_board = CURRENT_BOARD.read();
        if let Some(key) = current_board.owner_key {
            let boards = BOARDS.read();
            if let Some(board_data) = boards.map.get(&key) {
                board_data.can_create_categories()
            } else {
                false
            }
        } else {
            false
        }
    };

    if !can_create {
        return rsx! {
            // Backdrop
            div {
                class: "fixed inset-0 bg-black/50 z-40",
                onclick: move |_| {
                    CREATE_CATEGORY_MODAL.with_mut(|modal| {
                        modal.show = false;
                    });
                }
            }
            // Error modal
            div { class: "fixed inset-0 z-50 flex items-center justify-center p-4",
                div {
                    class: "bg-panel rounded-xl shadow-xl max-w-md w-full p-6 text-center",
                    onclick: move |e| e.stop_propagation(),
                    h2 { class: "text-lg font-semibold text-text mb-4", "Permission Denied" }
                    p { class: "text-text-muted mb-4",
                        "Only board owners and admins can create categories."
                    }
                    button {
                        class: "px-4 py-2 bg-accent hover:bg-accent/80 text-white rounded-lg transition-colors",
                        onclick: move |_| {
                            CREATE_CATEGORY_MODAL.with_mut(|modal| {
                                modal.show = false;
                            });
                        },
                        "Close"
                    }
                }
            }
        };
    }

    let create_category = move |_| {
        let name_val = name.read().trim().to_string();
        if name_val.is_empty() {
            error_msg.set(Some("Category name cannot be empty".to_string()));
            return;
        }

        if name_val.len() > 50 {
            error_msg.set(Some("Category name must be 50 characters or less".to_string()));
            return;
        }

        let icon_val = {
            let ic = icon.read().trim().to_string();
            if ic.is_empty() {
                None
            } else {
                if let Err(e) = validate_icon(&ic) {
                    error_msg.set(Some(e));
                    return;
                }
                Some(ic)
            }
        };

        error_msg.set(None);
        is_creating.set(true);

        let description_val = {
            let desc = description.read().trim().to_string();
            if desc.is_empty() {
                None
            } else {
                Some(desc)
            }
        };

        let color_val = Some(color.read().clone());
        let parent_id_val = parent_id_for_closure.clone();

        spawn_local(async move {
            match send_category(name_val, description_val, icon_val, color_val, parent_id_val).await
            {
                Ok(()) => {
                    // Reset and close modal
                    name.set(String::new());
                    description.set(String::new());
                    icon.set(String::new());
                    color.set(COLOR_PRESETS[0].0.to_string());
                    CREATE_CATEGORY_MODAL.with_mut(|modal| {
                        modal.show = false;
                        modal.parent_category_id = None;
                    });
                }
                Err(e) => {
                    error_msg.set(Some(format!("Failed to create category: {}", e)));
                }
            }
            is_creating.set(false);
        });
    };

    let title = if has_parent {
        "Create Subcategory"
    } else {
        "Create Category"
    };

    // Get parent category name for display
    let parent_category_name = if has_parent {
        CURRENT_CATEGORY.read().category_name.clone()
    } else {
        None
    };

    rsx! {
        // Backdrop
        div {
            class: "fixed inset-0 bg-black/50 z-40",
            onclick: move |_| {
                CREATE_CATEGORY_MODAL.with_mut(|modal| {
                    modal.show = false;
                    modal.parent_category_id = None;
                });
            }
        }

        // Modal
        div { class: "fixed inset-0 z-50 flex items-center justify-center p-4",
            div {
                class: "bg-panel rounded-xl shadow-xl max-w-md w-full",
                onclick: move |e| e.stop_propagation(),

                // Header
                div { class: "px-6 py-4 border-b border-border",
                    h2 { class: "text-lg font-semibold text-text", "{title}" }
                }

                // Body
                div { class: "px-6 py-4 space-y-4",
                    // Error message
                    if let Some(err) = error_msg.read().as_ref() {
                        div { class: "p-3 bg-error-bg border border-red-200 dark:border-red-800 rounded-lg text-sm text-red-700 dark:text-red-400",
                            "{err}"
                        }
                    }

                    // Parent category indicator
                    if let Some(parent_name) = parent_category_name.as_ref() {
                        div { class: "flex items-center gap-2 px-3 py-2 bg-surface border-l-2 border-accent rounded text-sm text-text-muted",
                            span { class: "flex-1",
                                span { class: "font-medium", "📁 Creating in: " }
                                "{parent_name}"
                            }
                        }
                    }

                    // Name input
                    div {
                        label { class: "block text-sm font-medium text-text mb-1", "Name *" }
                        input {
                            class: "w-full px-3 py-2 bg-surface border border-border rounded-lg text-text placeholder-text-muted focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent",
                            value: "{name}",
                            placeholder: "Category name",
                            maxlength: 50,
                            oninput: move |evt| name.set(evt.value().replace(['\n', '\r'], ""))
                        }
                    }

                    // Description input
                    div {
                        label { class: "block text-sm font-medium text-text mb-1", "Description" }
                        textarea {
                            class: "w-full px-3 py-2 bg-surface border border-border rounded-lg text-text placeholder-text-muted focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent resize-y min-h-[80px]",
                            value: "{description}",
                            placeholder: "Optional description",
                            oninput: move |evt| description.set(evt.value())
                        }
                    }

                    // Icon input
                    div {
                        label { class: "block text-sm font-medium text-text mb-1", "Icon (emoji)" }
                        div { class: "flex items-center gap-3",
                            input {
                                class: "flex-1 px-3 py-2 bg-surface border border-border rounded-lg text-text placeholder-text-muted focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent",
                                value: "{icon}",
                                placeholder: "Type an emoji (optional)",
                                oninput: move |evt| icon.set(evt.value())
                            }
                            // Preview
                            div {
                                class: "w-10 h-10 flex items-center justify-center bg-surface rounded-lg border border-border text-2xl",
                                {
                                    let ic = icon.read().clone();
                                    if ic.is_empty() { "📁".to_string() } else { ic }
                                }
                            }
                        }
                    }

                    // Color picker
                    div {
                        label { class: "block text-sm font-medium text-text mb-2", "Color" }
                        div { class: "flex flex-wrap gap-2",
                            {COLOR_PRESETS.iter().map(|(hex, label)| {
                                let hex = *hex;
                                let is_selected = *color.read() == hex;
                                rsx! {
                                    button {
                                        key: "{hex}",
                                        class: if is_selected {
                                            "w-8 h-8 rounded-full ring-2 ring-offset-2 ring-offset-panel ring-text"
                                        } else {
                                            "w-8 h-8 rounded-full hover:scale-110 transition-transform"
                                        },
                                        style: "background-color: {hex};",
                                        title: "{label}",
                                        onclick: move |_| color.set(hex.to_string()),
                                    }
                                }
                            })}
                        }
                    }
                }

                // Footer
                div { class: "px-6 py-4 border-t border-border flex justify-end gap-3",
                    button {
                        class: "px-4 py-2 text-sm text-text-muted hover:text-text hover:bg-surface rounded-lg transition-colors",
                        disabled: *is_creating.read(),
                        onclick: move |_| {
                            CREATE_CATEGORY_MODAL.with_mut(|modal| {
                                modal.show = false;
                                modal.parent_category_id = None;
                            });
                        },
                        "Cancel"
                    }
                    button {
                        class: "px-4 py-2 text-sm bg-accent hover:bg-accent/80 text-white rounded-lg transition-colors disabled:opacity-50 disabled:cursor-not-allowed",
                        disabled: *is_creating.read() || name.read().trim().is_empty(),
                        onclick: create_category,
                        if *is_creating.read() { "Creating..." } else { "Create" }
                    }
                }
            }
        }
    }
}
