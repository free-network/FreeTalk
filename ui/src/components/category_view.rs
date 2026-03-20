use crate::components::conversation::MessageData;
use dioxus::prelude::*;

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
