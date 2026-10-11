//! HTML sanitizing: no scripts, no remote resources (trackers and latency), links kept.

use std::borrow::Cow;

/// Returns HTML safe to render in a JS-disabled web view; remote images are dropped
/// so opening a message never phones home and never waits on the network.
pub fn sanitize_html(html: &str) -> String {
    ammonia::Builder::default()
        .add_generic_attributes(["style", "align", "width", "height", "bgcolor", "color"])
        .add_tags(["center", "font"])
        .attribute_filter(|element, attribute, value| {
            let remote = value.starts_with("http:")
                || value.starts_with("https:")
                || value.starts_with("//");
            match (element, attribute) {
                ("img", "src") | ("img", "srcset") if remote => None,
                (_, "style") if value.contains("url(") => None,
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
