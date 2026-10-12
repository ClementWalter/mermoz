//! Inline images: `<img src="cid:…">` in a message's HTML points to one of its own MIME
//! parts. They are resolved locally to `data:` URIs, so pasted images show in the body
//! with no network and are never treated as remote (tracking) images.

use base64::Engine;

/// One of the message's own parts an HTML body can point to.
#[derive(Debug, Clone, PartialEq)]
pub struct InlinePart {
    pub content_id: Option<String>,
    pub filename: String,
    pub mime: String,
    pub data: Vec<u8>,
}

/// Parts above this stay out of the HTML (it is shipped to the reading pane whole).
const MAX_INLINE_BYTES: usize = 8 * 1024 * 1024;

/// "<Image1.PNG@01D2>" or "cid:image1.png%4001d2" → "image1.png@01d2".
pub fn normalize_cid(raw: &str) -> String {
    let s = raw.trim();
    let s = s
        .strip_prefix("cid:")
        .or_else(|| s.strip_prefix("CID:"))
        .unwrap_or(s);
    percent_decode(s.trim().trim_start_matches('<').trim_end_matches('>')).to_lowercase()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && b[i + 1].is_ascii_hexdigit()
            && b[i + 2].is_ascii_hexdigit()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Which part a `cid:` reference (and the image's alt text) points to: the Content-ID
/// first, then a part named like the reference's local part or like the alt text.
fn find<'a>(parts: &'a [InlinePart], cid: &str, alt: Option<&str>) -> Option<&'a InlinePart> {
    let local = cid.split('@').next().unwrap_or(cid);
    parts
        .iter()
        .find(|p| p.content_id.as_deref().map(normalize_cid).as_deref() == Some(cid))
        .or_else(|| parts.iter().find(|p| p.filename.to_lowercase() == local))
        .or_else(|| {
            let alt = alt?.trim().to_lowercase();
            parts
                .iter()
                .find(|p| !alt.is_empty() && p.filename.to_lowercase() == alt)
        })
}

fn data_uri(p: &InlinePart) -> String {
    format!(
        "data:{};base64,{}",
        p.mime,
        base64::engine::general_purpose::STANDARD.encode(&p.data)
    )
}

/// Value of `name="…"` (or '…', or bare) inside one tag.
fn attr<'t>(tag: &'t str, name: &str) -> Option<(usize, usize, &'t str)> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        from = at + name.len();
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = lower[at + name.len()..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let eq = at + name.len() + (lower[at + name.len()..].len() - rest.len());
        let v = tag[eq + 1..].trim_start();
        let vstart = tag.len() - v.len();
        let (start, end) = match v.chars().next() {
            Some(q @ ('"' | '\'')) => {
                let close = v[1..].find(q)? + 1;
                (vstart + 1, vstart + close)
            }
            _ => {
                let end = v
                    .find(|c: char| c.is_whitespace() || c == '>')
                    .unwrap_or(v.len());
                (vstart, vstart + end)
            }
        };
        return Some((start, end, &tag[start..end]));
    }
    None
}

/// Replaces every `<img src="cid:…">` that matches one of `parts` with its data. Returns
/// the HTML and how many references found no part.
pub fn resolve(html: &str, parts: &[InlinePart]) -> (String, usize) {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let (mut last, mut missing) = (0, 0);
    let mut search = 0;
    while let Some(i) = lower[search..].find("<img") {
        let start = search + i;
        let Some(len) = lower[start..].find('>') else {
            break;
        };
        let end = start + len;
        search = end;
        let tag = &html[start..end];
        let Some((vs, ve, src)) = attr(tag, "src") else {
            continue;
        };
        if !src.trim_start().to_ascii_lowercase().starts_with("cid:") {
            continue;
        }
        let alt = attr(tag, "alt").map(|(_, _, a)| a);
        match find(parts, &normalize_cid(src), alt).filter(|p| p.data.len() <= MAX_INLINE_BYTES) {
            Some(p) => {
                out.push_str(&html[last..start + vs]);
                out.push_str(&data_uri(p));
                last = start + ve;
            }
            None => missing += 1,
        }
    }
    out.push_str(&html[last..]);
    (out, missing)
}

/// Whether the HTML points to any of its own parts.
pub fn has_cid(html: &str) -> bool {
    html.to_ascii_lowercase().contains("cid:")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn part(cid: Option<&str>, name: &str) -> InlinePart {
        InlinePart {
            content_id: cid.map(String::from),
            filename: name.into(),
            mime: "image/png".into(),
            data: b"PNG".to_vec(),
        }
    }

    const DATA: &str = "data:image/png;base64,UE5H";

    #[rstest]
    #[case::angle_brackets("<ii_k1@mail.example>", "ii_k1@mail.example")]
    #[case::prefixed("cid:ii_k1@mail.example", "ii_k1@mail.example")]
    #[case::url_encoded("cid:image%40mail.example", "image@mail.example")]
    #[case::non_ascii("cid:Graphique%20Coll%C3%A9-1.png", "graphique collé-1.png")]
    #[case::upper("CID:IMG1", "img1")]
    fn cids_normalize(#[case] raw: &str, #[case] want: &str) {
        assert_eq!(normalize_cid(raw), want);
    }

    #[rstest]
    fn content_id_resolves() {
        let (html, _) = resolve(
            r#"<p><img src="cid:ii_k1"></p>"#,
            &[part(Some("<ii_k1>"), "a.png")],
        );
        assert!(html.contains(DATA));
    }

    #[rstest]
    fn url_encoded_reference_resolves() {
        let (html, _) = resolve(
            r#"<img src="cid:Graphique%20Coll%C3%A9-1.png@01D2">"#,
            &[part(Some("Graphique Collé-1.png@01D2"), "x.png")],
        );
        assert!(html.contains(DATA));
    }

    #[rstest]
    fn file_name_resolves_without_a_content_id() {
        let (html, _) = resolve(
            r#"<img src="cid:chart.png@01D2">"#,
            &[part(None, "Chart.png")],
        );
        assert!(html.contains(DATA));
    }

    #[rstest]
    fn alt_text_resolves_a_pasted_image() {
        let (html, _) = resolve(
            r#"<img alt="GraphiqueCollé-1.png" src="cid:ii_abc">"#,
            &[part(None, "GraphiqueCollé-1.png")],
        );
        assert!(html.contains(DATA));
    }

    #[rstest]
    fn single_quotes_resolve() {
        let (html, _) = resolve("<img src='cid:ii_k1' />", &[part(Some("ii_k1"), "a.png")]);
        assert!(html.contains(DATA));
    }

    #[rstest]
    fn unknown_references_are_counted() {
        assert_eq!(
            resolve(r#"<img src="cid:nope">"#, &[part(Some("ii_k1"), "a.png")]).1,
            1
        );
    }

    #[rstest]
    fn remote_images_are_left_alone() {
        let html = r#"<img src="https://cdn.example/a.png">"#;
        assert_eq!(resolve(html, &[part(Some("a"), "a.png")]).0, html);
    }

    #[rstest]
    fn text_around_images_is_kept() {
        let (html, _) = resolve(
            r#"before <img src="cid:k"> after"#,
            &[part(Some("k"), "a.png")],
        );
        assert!(html.starts_with("before <img src=\"data:") && html.ends_with("\"> after"));
    }

    #[rstest]
    #[case::has(r#"<img src="cid:x">"#, true)]
    #[case::none(r#"<img src="https://x">"#, false)]
    fn cid_detection(#[case] html: &str, #[case] want: bool) {
        assert_eq!(has_cid(html), want);
    }
}
