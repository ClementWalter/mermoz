//! HTML sanitizing: no scripts; remote images only when asked for (they can track
//! opens and cost network time); links kept.

use std::borrow::Cow;

/// Returns HTML safe to render in a JS-disabled web view; remote images are dropped
/// so opening a message never phones home and never waits on the network.
pub fn sanitize_html(html: &str) -> String {
    sanitize_html_with(html, false)
}

/// Whether a message's HTML references remote images (hidden unless loaded).
pub fn has_remote_images(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("<img")
        && (lower.contains("src=\"http")
            || lower.contains("src='http")
            || lower.contains("src=http")
            || lower.contains("src=\"//"))
        || lower.contains("url(http")
}

/// Like [`sanitize_html`], keeping remote images when `images` is true (the reader
/// chose to load them, or the sender is verified).
pub fn sanitize_html_with(html: &str, images: bool) -> String {
    ammonia::Builder::default()
        .add_generic_attributes(["style", "align", "width", "height", "bgcolor", "color"])
        .add_tags(["center", "font"])
        .attribute_filter(move |element, attribute, value| {
            let remote = value.starts_with("http:")
                || value.starts_with("https:")
                || value.starts_with("//");
            match (element, attribute) {
                ("img", "src") | ("img", "srcset") if remote && !images => None,
                (_, "style") if value.contains("url(") && !images => None,
                _ => Some(Cow::Borrowed(value)),
            }
        })
        .link_rel(Some("noopener noreferrer"))
        .clean(html)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    fn remote_images_load_when_asked() {
        assert!(
            sanitize_html_with(r#"<img src="https://cdn.example/logo.png">"#, true)
                .contains("logo.png")
        );
    }

    #[rstest]
    #[case::remote_image(r#"<img src="https://cdn.example/a.png">"#, true)]
    #[case::inline_data(r#"<img src="data:image/png;base64,AAAA">"#, false)]
    #[case::no_image("<p>hi</p>", false)]
    fn remote_images_are_detected(#[case] html: &str, #[case] want: bool) {
        assert_eq!(has_remote_images(html), want);
    }

    #[rstest]
    fn scripts_are_removed() {
        assert!(!sanitize_html("<p>a</p><script>alert(1)</script>").contains("script"));
    }

    #[rstest]
    fn remote_images_are_removed() {
        assert!(!sanitize_html(r#"<img src="https://t.example/pixel.gif">"#).contains("pixel"));
    }

    #[rstest]
    fn links_are_kept() {
        assert!(
            sanitize_html(r#"<a href="https://example.com">x</a>"#).contains("https://example.com")
        );
    }

    #[rstest]
    fn css_urls_are_removed() {
        assert!(
            !sanitize_html(r#"<div style="background:url(https://t.example/x)">a</div>"#)
                .contains("t.example")
        );
    }
}
