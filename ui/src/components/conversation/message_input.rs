use dioxus::prelude::*;
use dioxus_free_icons::icons::fa_solid_icons::{FaPen, FaXmark};
use dioxus_free_icons::Icon;

use super::emoji_picker::EmojiPicker;
use super::ReplyContext;

/// Post input component with a compose button that opens a modal.
/// The modal contains fields for title and content.
#[component]
pub fn PostInput(
    handle_send_message: EventHandler<(String, String, Option<ReplyContext>)>,
    replying_to: Signal<Option<ReplyContext>>,
    on_request_edit_last: EventHandler<()>,
    /// Default reply context - applied when modal opens if replying_to is None
    #[props(default)]
    default_reply_to: Option<ReplyContext>,
    /// Maximum title length (from board configuration)
    max_title_size: usize,
    /// Maximum message length (from board configuration)
    max_message_size: usize,
) -> Element {
    let mut show_modal = use_signal(|| false);
    let mut title_text = use_signal(String::new);
    let mut message_text = use_signal(String::new);
    let mut show_emoji_picker = use_signal(|| false);

    // Open modal when replying_to changes to Some (from clicking reply button)
    use_effect(move || {
        if replying_to.read().is_some() && !show_modal() {
            show_modal.set(true);
        }
    });

    // Handler for opening the modal via Compose button
    let open_modal = {
        let default_reply = default_reply_to.clone();
        move |_| {
            // If no explicit reply is set but we have a default, use it
            if replying_to.peek().is_none() {
                if let Some(ref default) = default_reply {
                    replying_to.set(Some(default.clone()));
                }
            }
            show_modal.set(true);
        }
    };

    let mut send_message = move || {
        let title = title_text.peek().to_string();
        let content = message_text.peek().to_string();
        if !content.is_empty() {
            let reply_ctx = replying_to.peek().clone();
            title_text.set(String::new());
            message_text.set(String::new());
            replying_to.set(None);
            show_modal.set(false);
            handle_send_message.call((title, content, reply_ctx));
        }
    };

    // Handle emoji selection - insert at end of message
    let handle_emoji_select = move |emoji: String| {
        let current = message_text.peek().to_string();
        message_text.set(format!("{}{}", current, emoji));
    };

    rsx! {
        // Compose button bar
        button {
            class: "p-3 bg-accent hover:bg-accent-hover text-white rounded-xl transition-colors",
            onclick: open_modal,
            title: if default_reply_to.is_some() { "Compose Reply" } else { "Compose Post" },
            Icon { icon: FaPen, width: 18, height: 18 }
        }

        // Compose modal
        if show_modal() {
            div {
                class: "fixed inset-0 bg-black/50 flex items-center justify-center z-50",
                onclick: move |_| {
                    show_modal.set(false);
                    replying_to.set(None);
                },
                div {
                    class: "bg-panel rounded-xl shadow-xl w-full max-w-2xl mx-4 max-h-[90vh] flex flex-col",
                    onclick: move |e| e.stop_propagation(),

                    // Modal header
                    div { class: "flex items-center justify-between px-6 py-4 border-b border-border",
                        h2 { class: "text-lg font-semibold text-text",
                            if replying_to.read().is_some() { "Reply to Post" } else { "New Post" }
                        }
                        button {
                            class: "p-2 rounded-lg text-text-muted hover:text-text hover:bg-surface transition-colors",
                            onclick: move |_| {
                                show_modal.set(false);
                                replying_to.set(None);
                            },
                            Icon { icon: FaXmark, width: 16, height: 16 }
                        }
                    }

                    // Modal body
                    div { class: "flex-1 overflow-y-auto px-6 py-4 space-y-4",
                        // Reply preview strip
                        {
                            let reply = replying_to.read();
                            if let Some(ctx) = reply.as_ref() {
                                let author = ctx.author_name.clone();
                                let preview = ctx.content_preview.clone();
                                rsx! {
                                    div { class: "flex items-center gap-2 px-3 py-2 bg-surface border-l-2 border-accent rounded text-sm text-text-muted",
                                        span { class: "flex-1 truncate",
                                            span { class: "font-medium", "\u{21a9} Replying to @{author}: " }
                                            "{preview}"
                                        }
                                        button {
                                            class: "text-text-muted hover:text-text transition-colors flex-shrink-0",
                                            title: "Cancel reply",
                                            onclick: move |_| replying_to.set(None),
                                            "\u{00d7}"
                                        }
                                    }
                                }
                            } else {
                                rsx! {}
                            }
                        }

                        // Title field
                        div { class: "space-y-1.5",
                            div { class: "flex items-center justify-between",
                                label { class: "block text-sm font-medium text-text",
                                    "Title"
                                    span { class: "text-text-muted font-normal", " (optional)" }
                                }
                                span {
                                    class: if title_text.read().len() > max_title_size { "text-xs text-red-400" } else { "text-xs text-text-muted" },
                                    "{title_text.read().len()}/{max_title_size}"
                                }
                            }
                            input {
                                r#type: "text",
                                class: "w-full px-4 py-2.5 bg-surface border border-border rounded-lg text-text placeholder-text-muted focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent transition-colors",
                                placeholder: "Add a title...",
                                maxlength: max_title_size as i64,
                                value: "{title_text}",
                                oninput: move |evt| {
                                    // Strip newlines from title
                                    let value = evt.value().replace(['\n', '\r'], "");
                                    title_text.set(value);
                                },
                            }
                        }

                        // Content field
                        div { class: "space-y-1.5",
                            div { class: "flex items-center justify-between",
                                label { class: "block text-sm font-medium text-text",
                                    "Post"
                                }
                                span {
                                    class: if message_text.read().len() > max_message_size { "text-xs text-red-400" } else { "text-xs text-text-muted" },
                                    "{message_text.read().len()}/{max_message_size}"
                                }
                            }
                            div { class: "relative",
                                // Emoji picker backdrop
                                if show_emoji_picker() {
                                    div {
                                        class: "fixed inset-0 z-40",
                                        onclick: move |_| show_emoji_picker.set(false),
                                    }
                                }
                                div { class: "flex gap-2",
                                    // Emoji picker button
                                    div { class: "relative self-start pt-2",
                                        button {
                                            class: "p-2 rounded-lg hover:bg-surface transition-colors",
                                            title: "Insert emoji",
                                            onclick: move |_| show_emoji_picker.set(!show_emoji_picker()),
                                            span {
                                                class: "text-lg",
                                                style: "filter: grayscale(100%); opacity: 0.6;",
                                                "🙂"
                                            }
                                        }
                                        if show_emoji_picker() {
                                            div {
                                                class: "absolute bottom-full left-0 mb-2 z-50",
                                                EmojiPicker {
                                                    on_select: handle_emoji_select,
                                                    on_close: move |_| show_emoji_picker.set(false),
                                                }
                                            }
                                        }
                                    }
                                    textarea {
                                        id: "message-input",
                                        class: "flex-1 px-4 py-2.5 bg-surface border border-border rounded-lg text-text placeholder-text-muted focus:outline-none focus:ring-2 focus:ring-accent/50 focus:border-accent transition-colors resize-none min-h-[120px]",
                                        placeholder: "Type your message...",
                                        value: "{message_text}",
                                        rows: "5",
                                        oninput: move |evt| message_text.set(evt.value().to_string()),
                                        onkeydown: move |evt| {
                                            // Ctrl/Cmd+Enter sends the message
                                            if evt.key() == Key::Enter && (evt.modifiers().ctrl() || evt.modifiers().meta()) {
                                                evt.prevent_default();
                                                send_message();
                                            }
                                            // Escape closes modal
                                            if evt.key() == Key::Escape {
                                                show_modal.set(false);
                                                replying_to.set(None);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Modal footer - wrapped in form for iPad virtual keyboard support
                    form {
                        class: "flex items-center justify-between px-6 py-4 border-t border-border bg-surface/50",
                        onsubmit: move |evt| {
                            evt.prevent_default();
                            send_message();
                        },
                        span { class: "text-xs text-text-muted",
                            "Press Ctrl+Enter to send"
                        }
                        div { class: "flex gap-3",
                            button {
                                r#type: "button",
                                class: "px-4 py-2 rounded-lg bg-surface hover:bg-surface-hover text-text transition-colors",
                                onclick: move |_| {
                                    show_modal.set(false);
                                    replying_to.set(None);
                                },
                                "Cancel"
                            }
                            button {
                                r#type: "submit",
                                class: "px-5 py-2 bg-accent hover:bg-accent-hover text-white font-medium rounded-lg transition-colors disabled:opacity-50 disabled:cursor-not-allowed",
                                disabled: message_text.read().is_empty() || message_text.read().len() > max_message_size || title_text.read().len() > max_title_size,
                                "Send Post"
                            }
                        }
                    }
                }
            }
        }
    }
}
