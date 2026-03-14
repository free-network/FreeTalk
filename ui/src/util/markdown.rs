/// Utilities for converting text to HTML with markdown and URL linkification.

/// Convert message text to HTML with clickable links that open in new tabs.
///
/// Uses GFM autolink literals to linkify plain URLs while correctly
/// skipping URLs inside code spans and other non-text contexts.
pub fn text_to_html(text: &str) -> String {
    // Convert single newlines to hard breaks (two spaces + newline)
    // This preserves line breaks in chat messages as users expect
    let with_hard_breaks = text.replace("\n", "  \n");

    // Convert markdown to HTML using GFM mode, which includes autolink
    // literals that correctly handle code spans, existing links, etc.
    let html = markdown::to_html_with_options(&with_hard_breaks, &markdown::Options::gfm())
        .unwrap_or_else(|_| markdown::to_html(&with_hard_breaks));

    // Add target="_blank" and rel="noopener noreferrer" to all links
    make_links_open_in_new_tab(&html)
}

/// Add target="_blank" and rel="noopener noreferrer" to all anchor tags in HTML.
pub fn make_links_open_in_new_tab(html: &str) -> String {
    html.replace(
        "<a href=\"",
        "<a target=\"_blank\" rel=\"noopener noreferrer\" href=\"",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_url_is_linkified() {
        let html = text_to_html("check out https://freenet.org for more info");
        assert!(
            html.contains(
                r#"<a target="_blank" rel="noopener noreferrer" href="https://freenet.org">"#
            ),
            "bare URL should be linkified with target=_blank: {html}"
        );
    }

    #[test]
    fn url_in_code_span_not_linkified() {
        let html = text_to_html("did you do `curl -fsSL https://freenet.org/install.sh | sh`?");
        assert!(
            !html.contains("<a "),
            "URL inside backticks should NOT be linkified: {html}"
        );
    }

    #[test]
    fn url_in_fenced_code_block_not_linkified() {
        let html = text_to_html("```\ncurl https://freenet.org/install.sh\n```");
        assert!(
            !html.contains("<a "),
            "URL in code block should NOT be linkified: {html}"
        );
    }

    #[test]
    fn markdown_link_preserved() {
        let html = text_to_html("see [Freenet](https://freenet.org)");
        assert!(
            html.contains(r#"href="https://freenet.org">"#),
            "markdown link should be preserved: {html}"
        );
        assert!(
            html.contains(">Freenet</a>"),
            "markdown link text should be preserved: {html}"
        );
    }

    #[test]
    fn newlines_become_hard_breaks() {
        let html = text_to_html("line one\nline two");
        assert!(
            html.contains("<br"),
            "newlines should become hard breaks: {html}"
        );
    }
}
