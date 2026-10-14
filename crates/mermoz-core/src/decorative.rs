//! Which images in an email are decoration (signature logos, social icons, banners,
//! tracking pixels) rather than content (a photo someone attached or pasted). The chat
//! shows and counts content images only.

/// What is known of one image of a message.
#[derive(Debug, Clone, Copy, Default)]
pub struct Image<'a> {
    pub filename: &'a str,
    pub mime: &'a str,
    pub size: i64,
    /// Placed in the body (`Content-Disposition: inline` or referenced by `cid:`).
    pub inline: bool,
    pub content_id: Option<&'a str>,
    /// The file's first bytes, enough for its width and height; empty when unknown.
    pub head: &'a [u8],
}

/// Below this many bytes an image is an icon, a bullet or a pixel.
const TINY_BYTES: i64 = 8 * 1024;
/// An image this narrow or short on either side is a logo, icon or rule.
const SMALL_SIDE: u32 = 120;

/// Names signature and newsletter images go by.
const NAMES: &[&str] = &[
    "logo",
    "facebook",
    "linkedin",
    "twitter",
    "youtube",
    "instagram",
    "tiktok",
    "pinterest",
    "icon",
    "signature",
    "banner",
    "bandeau",
    "pixel",
    "spacer",
    "social",
];

/// A name says decoration only for an image placed in the body or a small one: a
/// photo someone attached keeps its name ("facebook-party.jpg") and stays content.
const NAMED_MAX_BYTES: i64 = 50 * 1024;

/// A name a mail client made up ("image001.png", "image.png", "attachment",
/// "Outlook-x1y2.png") rather than one a person or a camera gave.
fn is_generic_name(lower: &str) -> bool {
    let stem = lower.rsplit_once('.').map_or(lower, |(s, _)| s);
    stem == "image"
        || stem == "attachment"
        || stem.starts_with("outlook-")
        || stem
            .strip_prefix("image")
            .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
}

/// An image's width and height from its first bytes (PNG, GIF, JPEG, WebP).
pub fn dimensions(b: &[u8]) -> Option<(u32, u32)> {
    let be32 = |i: usize| -> Option<u32> {
        b.get(i..i + 4)
            .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    };
    let be16 = |i: usize| -> Option<u32> {
        b.get(i..i + 2)
            .map(|s| u16::from_be_bytes([s[0], s[1]]) as u32)
    };
    let le16 = |i: usize| -> Option<u32> {
        b.get(i..i + 2)
            .map(|s| u16::from_le_bytes([s[0], s[1]]) as u32)
    };
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((be32(16)?, be32(20)?));
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some((le16(6)?, le16(8)?));
    }
    if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        // VP8X carries 24-bit sizes minus one; simple VP8/VP8L are rarer in mail.
        if b.get(12..16) == Some(b"VP8X") {
            let w = u32::from_le_bytes([b[24], b[25], b[26], 0]) + 1;
            let h = u32::from_le_bytes([b[27], b[28], b[29], 0]) + 1;
            return Some((w, h));
        }
        return None;
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        // Walk the segments to the frame header (SOF0..SOF15 but DHT/JPG/DAC).
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF {
                return None;
            }
            let marker = b[i + 1];
            let len = be16(i + 2)? as usize;
            if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
                return Some((be16(i + 7)?, be16(i + 5)?));
            }
            i += 2 + len;
        }
    }
    None
}

/// Whether the image's `cid:` link sits only in quoted history or the signature part of
/// the HTML (at or after `content_end`, the byte where the sender's own words end).
fn referenced_only_after(html: &str, content_id: &str, content_end: usize) -> bool {
    let cid = content_id.trim_matches(['<', '>']);
    if cid.is_empty() {
        return false;
    }
    let needle = format!("cid:{cid}");
    let mut found = false;
    for (pos, _) in html.match_indices(&needle) {
        if pos < content_end {
            return false;
        }
        found = true;
    }
    found
}

/// Whether `img` is decoration. `html` is the message's HTML and `content_end` the byte
/// where its new words end (the quote marker, else the whole body); `repeats` counts
/// the other messages (or copies in this one) where this sender sent the same image
/// (same size and type).
pub fn is_decorative(img: &Image, html: Option<&str>, content_end: usize, repeats: usize) -> bool {
    if !img.mime.to_ascii_lowercase().starts_with("image/") {
        return false;
    }
    if img.size > 0 && img.size < TINY_BYTES {
        return true;
    }
    if dimensions(img.head).is_some_and(|(w, h)| w <= SMALL_SIDE || h <= SMALL_SIDE) {
        return true;
    }
    let name = img.filename.to_ascii_lowercase();
    if (img.inline || img.size < NAMED_MAX_BYTES) && NAMES.iter().any(|n| name.contains(n)) {
        return true;
    }
    // A signature image comes back with every message: once is enough for one placed
    // in the body; a file sent along must come back twice more (older mail stored its
    // signature images as files). Only images that look like a signature's count: a
    // photo sent again ("passport.jpg") is still a photo.
    let generic = is_generic_name(&name);
    let signature_like = if img.inline {
        img.size < 200_000 || generic
    } else {
        generic && img.size < 1_000_000
    };
    if signature_like && (img.inline && repeats >= 1 || repeats >= 2) {
        return true;
    }
    match (html, img.content_id) {
        (Some(h), Some(cid)) => referenced_only_after(h, cid, content_end),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// The first bytes of a PNG of the given size.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b
    }

    fn gif(w: u16, h: u16) -> Vec<u8> {
        let mut b = b"GIF89a".to_vec();
        b.extend(w.to_le_bytes());
        b.extend(h.to_le_bytes());
        b
    }

    fn jpeg(w: u16, h: u16) -> Vec<u8> {
        let mut b = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        b.extend([0xFF, 0xC0, 0x00, 0x11, 0x08]);
        b.extend(h.to_be_bytes());
        b.extend(w.to_be_bytes());
        b.extend([0x03, 0, 0, 0, 0, 0, 0]);
        b
    }

    #[rstest]
    #[case::png(png(640, 480), Some((640, 480)))]
    #[case::gif(gif(1, 1), Some((1, 1)))]
    #[case::jpeg(jpeg(1200, 800), Some((1200, 800)))]
    #[case::unknown(b"not an image".to_vec(), None)]
    fn dimensions_come_from_the_header(#[case] head: Vec<u8>, #[case] want: Option<(u32, u32)>) {
        assert_eq!(dimensions(&head), want);
    }

    const PHOTO: Image = Image {
        filename: "IMG_2041.jpg",
        mime: "image/jpeg",
        size: 1_400_000,
        inline: false,
        content_id: None,
        head: &[],
    };

    #[rstest]
    fn an_attached_photo_is_content() {
        assert!(!is_decorative(&PHOTO, None, 0, 0));
    }

    #[rstest]
    fn a_pasted_screenshot_is_content() {
        let head = png(1280, 720);
        let img = Image {
            filename: "image001.png",
            mime: "image/png",
            size: 240_000,
            inline: true,
            content_id: Some("image001.png@01DA"),
            head: &head,
        };
        let html = r#"<p>Voici l'écran :</p><img src="cid:image001.png@01DA"><div id="appendonsend"></div>"#;
        assert!(!is_decorative(&img, Some(html), html.len(), 0));
    }

    #[rstest]
    #[case::tiny_icon(Image { filename: "image.png", mime: "image/png", size: 1206, inline: true, ..PHOTO })]
    #[case::named_logo(Image { filename: "logo-creche.jpg", size: 90_000, inline: true, ..PHOTO })]
    #[case::social_icon(Image { filename: "linkedin.png", mime: "image/png", size: 9_000, inline: true, ..PHOTO })]
    fn signature_images_are_decoration(#[case] img: Image) {
        assert!(is_decorative(&img, None, 0, 0));
    }

    #[rstest]
    #[case::tracking_pixel(gif(1, 1))]
    #[case::thin_banner(png(600, 90))]
    #[case::small_logo(png(110, 110))]
    fn small_images_are_decoration(#[case] head: Vec<u8>) {
        let img = Image {
            filename: "image.png",
            mime: "image/png",
            size: 20_000,
            inline: true,
            head: &head,
            ..PHOTO
        };
        assert!(is_decorative(&img, None, 0, 0));
    }

    #[rstest]
    fn a_banner_repeated_in_every_message_is_decoration() {
        let img = Image {
            filename: "image.png",
            size: 110_865,
            inline: true,
            ..PHOTO
        };
        assert!(is_decorative(&img, None, 0, 1));
    }

    #[rstest]
    fn an_attached_file_sent_twice_is_still_content() {
        assert!(!is_decorative(&PHOTO, None, 0, 1));
    }

    #[rstest]
    #[case::passport("passport.jpeg", 609_000)]
    #[case::camera_photo("IMG_0075.JPG", 826_000)]
    fn a_named_photo_sent_again_is_still_content(#[case] name: &str, #[case] size: i64) {
        let img = Image {
            filename: name,
            size,
            ..PHOTO
        };
        assert!(!is_decorative(&img, None, 0, 4));
    }

    #[rstest]
    fn a_large_pasted_screenshot_quoted_again_is_still_content() {
        let img = Image {
            filename: "Capture.png",
            mime: "image/png",
            size: 727_000,
            inline: true,
            ..PHOTO
        };
        assert!(!is_decorative(&img, None, 0, 3));
    }

    #[rstest]
    fn an_attached_signature_image_in_every_message_is_decoration() {
        let img = Image {
            filename: "image001.jpg",
            size: 195_000,
            ..PHOTO
        };
        assert!(is_decorative(&img, None, 0, 8));
    }

    #[rstest]
    fn an_image_only_in_the_quoted_history_is_decoration() {
        let img = Image {
            filename: "image.png",
            size: 60_000,
            inline: true,
            content_id: Some("<sig7@x>"),
            ..PHOTO
        };
        let html = r#"<p>Merci !</p><div class="gmail_quote"><img src="cid:sig7@x"></div>"#;
        let end = html.find("<div class=\"gmail_quote").unwrap();
        assert!(is_decorative(&img, Some(html), end, 0));
    }

    #[rstest]
    #[case::messaging_app_photo("WhatsApp Image 2026-06-24 at 10.09.40.jpeg")]
    #[case::social_named_photo("facebook-party.jpg")]
    fn an_attached_photo_named_after_an_app_is_content(#[case] name: &str) {
        let head = jpeg(720, 1520);
        let img = Image {
            filename: name,
            size: 103_000,
            head: &head,
            ..PHOTO
        };
        assert!(!is_decorative(&img, None, 0, 0));
    }

    #[rstest]
    fn a_document_is_never_decoration() {
        let pdf = Image {
            filename: "logo-charter.pdf",
            mime: "application/pdf",
            size: 2_000,
            ..PHOTO
        };
        assert!(!is_decorative(&pdf, None, 0, 3));
    }
}
