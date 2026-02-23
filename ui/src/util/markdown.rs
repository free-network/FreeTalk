/// Utilities for converting text to HTML with markdown and URL linkification.

/// Convert message text to HTML with clickable links that open in new tabs.
///
/// This function:
/// 1. Auto-linkifies plain URLs (http/https) that aren't already in markdown link syntax
/// 2. Converts markdown to HTML
/// 3. Adds target="_blank" rel="noopener noreferrer" to all links for security
pub fn text_to_html(text: &str) -> String {
    let linkified = auto_linkify_urls(text);
    let with_hard_breaks = linkified.replace("\n", "  \n");
    let html = markdown::to_html(&with_hard_breaks);
    make_links_open_in_new_tab(&html)
}

/// Auto-linkify plain URLs that aren't already in markdown link syntax.
/// Matches http:// and https:// URLs.
pub fn auto_linkify_urls(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        // Check if we're inside a markdown link [...](url) - skip the URL part
        if c == ']' {
            result.push(c);
            if let Some(&(_, '(')) = chars.peek() {
                // This is a markdown link, copy until closing paren
                result.push(chars.next().unwrap().1); // '('
                for (_, ch) in chars.by_ref() {
                    result.push(ch);
                    if ch == ')' {
                        break;
                    }
                }
            }
            continue;
        }

        // Check for URL start
        let remaining = &text[i..];
        if remaining.starts_with("http://") || remaining.starts_with("https://") {
            // Check if this URL is already inside a markdown link by looking back
            // for an unclosed '(' that follows ']'
            let before = &text[..i];
            let is_in_markdown_link = {
                let mut depth = 0i32;
                let mut in_link_url = false;
                for ch in before.chars().rev() {
                    if ch == ')' {
                        depth += 1;
                    } else if ch == '(' {
                        if depth > 0 {
                            depth -= 1;
                        } else {
                            in_link_url = true;
                            break;
                        }
                    } else if ch == ']' && depth == 0 {
                        // Found ']' before '(' - not in a link URL
                        break;
                    }
                }
                in_link_url
            };

            if is_in_markdown_link {
                result.push(c);
                continue;
            }

            // Extract the URL (until whitespace or certain punctuation at end)
            let url_end = remaining
                .find(|ch: char| ch.is_whitespace() || ch == '<' || ch == '>' || ch == '"')
                .unwrap_or(remaining.len());

            let mut url = &remaining[..url_end];

            // Trim trailing punctuation that's likely not part of the URL
            while url.ends_with(['.', ',', ';', ':', '!', '?', ')', ']']) {
                url = &url[..url.len() - 1];
            }

            // Create markdown link
            result.push('[');
            result.push_str(url);
            result.push_str("](");
            result.push_str(url);
            result.push(')');

            // Skip the URL characters we just processed
            for _ in 0..url.len() - 1 {
                chars.next();
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Add target="_blank" and rel="noopener noreferrer" to all anchor tags in HTML.
pub fn make_links_open_in_new_tab(html: &str) -> String {
    html.replace(
        "<a href=\"",
        "<a target=\"_blank\" rel=\"noopener noreferrer\" href=\"",
    )
}
