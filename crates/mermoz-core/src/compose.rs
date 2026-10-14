//! Outgoing mail: builds the RFC 5322 bytes for a draft, threaded when replying,
//! with files as base64 MIME parts and shared links listed under the text.

use crate::error::{Error, Result};
use mail_builder::MessageBuilder;
use mail_builder::encoders::base64::Base64Encoder;
use mail_builder::headers::content_type::ContentType;
use mail_builder::mime::{BodyPart, MimePart};

/// Gmail refuses to send more than 25 MB of attachments in one email.
pub const MAX_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;

/// A file carried by the message.
pub struct Part<'a> {
    pub filename: &'a str,
    pub mime: &'a str,
    pub bytes: &'a [u8],
    /// Shown in the body by mail apps rather than offered as a download.
    pub inline: bool,
}

/// Something shared by address instead of by bytes (e.g. a Drive file).
pub struct Link<'a> {
    pub title: &'a str,
    pub url: &'a str,
}

pub struct Outgoing<'a> {
    pub from: &'a str,
    pub to: &'a [String],
    pub cc: &'a [String],
    pub subject: &'a str,
    pub body: &'a str,
    pub in_reply_to: Option<&'a str>,
    pub references: &'a [String],
    pub parts: &'a [Part<'a>],
    pub links: &'a [Link<'a>],
}

fn strip_brackets(s: &str) -> String {
    s.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

/// "3.4 MB": what the user sees in limits and chips.
pub fn human_size(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= MB {
        format!("{:.1} MB", bytes as f64 / MB)
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

/// Refuses a total Gmail would bounce, in words the user can act on.
pub fn check_total(bytes: u64) -> Result<()> {
    if bytes > MAX_ATTACHMENT_BYTES {
        return Err(Error::InvalidParams(format!(
            "These files add up to {}, more than the {} Gmail can send in one email. Remove some files, or share the large ones with a link.",
            human_size(bytes),
            human_size(MAX_ATTACHMENT_BYTES)
        )));
    }
    Ok(())
}

/// Content type from the file extension, else from the first bytes, else generic binary.
pub fn mime_for(filename: &str, head: &[u8]) -> String {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    let by_ext = match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "ics" => "text/calendar",
        "vcf" => "text/vcard",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "pages" => "application/vnd.apple.pages",
        "numbers" => "application/vnd.apple.numbers",
        "key" => "application/vnd.apple.keynote",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "eml" => "message/rfc822",
        _ => "",
    };
    if !by_ext.is_empty() {
        return by_ext.into();
    }
    let sniffed = if head.starts_with(b"%PDF-") {
        "application/pdf"
    } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if head.starts_with(b"\xFF\xD8\xFF") {
        "image/jpeg"
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        "image/gif"
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        "image/webp"
    } else if head.starts_with(b"PK\x03\x04") {
        "application/zip"
    } else {
        "application/octet-stream"
    };
    sniffed.into()
}

/// One attachment part, always base64 so every byte survives relays untouched.
fn mime_part<'a>(p: &Part<'a>, index: usize) -> MimePart<'a> {
    let encoded = Base64Encoder::new()
        .wrap_lines()
        .encode(p.bytes)
        .unwrap_or_default();
    // `name` on Content-Type too: older Outlook versions only read that one.
    let ct = ContentType::new(p.mime.to_string()).attribute("name", p.filename.to_string());
    let disposition = ContentType::new(if p.inline { "inline" } else { "attachment" })
        .attribute("filename", p.filename.to_string());
    let mut part = MimePart::new(ct, BodyPart::Binary(encoded.into()))
        .header("Content-Disposition", disposition)
        .transfer_encoding("base64");
    if p.inline {
        part = part.cid(format!("part{index}@mermoz"));
    }
    part
}

/// Body text followed by the shared links, one per line.
fn body_with_links(body: &str, links: &[Link]) -> String {
    if links.is_empty() {
        return body.to_string();
    }
    let mut out = body.trim_end().to_string();
    out.push_str("\n\n");
    for l in links {
        out.push_str(&format!("{}: {}\n", l.title, l.url));
    }
    out
}

/// The reply text followed by the answered message, quoted the way mail apps do:
/// "On <date>, <name> <address> wrote:" then each of its lines after "> ".
pub fn with_quote(body: &str, answered: &crate::model::Message) -> String {
    let when = chrono::DateTime::from_timestamp_millis(answered.date)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%a, %b %-d, %Y at %H:%M")
                .to_string()
        })
        .unwrap_or_default();
    quote_block(body, &when, &answered.from, &answered.body_text)
}

/// [`with_quote`] with the date already in words.
pub fn quote_block(body: &str, when: &str, from: &crate::model::Address, text: &str) -> String {
    let who = if from.name.trim().is_empty() {
        format!("<{}>", from.email)
    } else {
        format!("{} <{}>", from.name.trim(), from.email)
    };
    let intro = if when.is_empty() {
        format!("{who} wrote:")
    } else {
        format!("On {when}, {who} wrote:")
    };
    let quoted: Vec<String> = text
        .replace("\r\n", "\n")
        .trim_end()
        .lines()
        .map(|l| {
            if l.is_empty() {
                ">".to_string()
            } else {
                format!("> {l}")
            }
        })
        .collect();
    format!("{}\n\n{intro}\n{}\n", body.trim_end(), quoted.join("\n"))
}

/// Plain-text message (multipart/mixed when it carries files); reply headers keep
/// it in the recipient's existing thread.
pub fn build(o: &Outgoing) -> Result<Vec<u8>> {
    if o.to.is_empty() && o.cc.is_empty() {
        return Err(Error::InvalidParams("draft has no recipients".into()));
    }
    check_total(o.parts.iter().map(|p| p.bytes.len() as u64).sum())?;
    let text = body_with_links(o.body, o.links);
    let mut b = MessageBuilder::new()
        .from(o.from.to_string())
        .to(o.to.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .subject(o.subject);
    if !o.cc.is_empty() {
        b = b.cc(o.cc.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    }
    if let Some(r) = o.in_reply_to {
        b = b.in_reply_to(strip_brackets(r));
    }
    if !o.references.is_empty() {
        b = b.references(
            o.references
                .iter()
                .map(|s| strip_brackets(s))
                .collect::<Vec<_>>(),
        );
    }
    b = if o.parts.is_empty() {
        b.text_body(text)
    } else {
        let mut children = vec![MimePart::new("text/plain", BodyPart::Text(text.into()))];
        children.extend(o.parts.iter().enumerate().map(|(i, p)| mime_part(p, i)));
        b.body(MimePart::new(
            "multipart/mixed",
            BodyPart::Multipart(children),
        ))
    };
    b.write_to_vec().map_err(|e| Error::other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mail_parser::{MessageParser, MimeHeaders};
    use rstest::{fixture, rstest};

    const PDF: &[u8] = b"%PDF-1.4 fictional";
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfictional\x00\xff";

    fn to() -> Vec<String> {
        vec!["marc@example.com".to_string()]
    }

    fn build_with(parts: &[Part], links: &[Link], reply: bool) -> Vec<u8> {
        let to = to();
        let refs = vec!["<a@x>".to_string()];
        build(&Outgoing {
            from: "me@example.com",
            to: &to,
            cc: &[],
            subject: "Re: Audit",
            body: "Looks good",
            in_reply_to: if reply { Some("<a@x>") } else { None },
            references: if reply { &refs } else { &[] },
            parts,
            links,
        })
        .unwrap()
    }

    #[fixture]
    fn sample() -> String {
        String::from_utf8(build_with(&[], &[], true)).unwrap()
    }

    #[fixture]
    fn one_file() -> Vec<u8> {
        build_with(
            &[Part {
                filename: "report.pdf",
                mime: "application/pdf",
                bytes: PDF,
                inline: false,
            }],
            &[],
            false,
        )
    }

    #[fixture]
    fn two_files() -> Vec<u8> {
        build_with(
            &[
                Part {
                    filename: "report.pdf",
                    mime: "application/pdf",
                    bytes: PDF,
                    inline: false,
                },
                Part {
                    filename: "chart.png",
                    mime: "image/png",
                    bytes: PNG,
                    inline: true,
                },
            ],
            &[],
            false,
        )
    }

    #[fixture]
    fn reply_with_file() -> String {
        String::from_utf8(build_with(
            &[Part {
                filename: "report.pdf",
                mime: "application/pdf",
                bytes: PDF,
                inline: false,
            }],
            &[],
            true,
        ))
        .unwrap()
    }

    fn parse(raw: &[u8]) -> mail_parser::Message<'_> {
        MessageParser::default().parse(raw).unwrap()
    }

    #[rstest]
    fn has_in_reply_to(sample: String) {
        assert!(sample.contains("In-Reply-To: <a@x>"));
    }

    #[rstest]
    fn has_body(sample: String) {
        assert!(sample.contains("Looks good"));
    }

    #[rstest]
    fn plain_message_is_not_multipart(sample: String) {
        assert!(!sample.contains("multipart/mixed"));
    }

    #[rstest]
    fn empty_recipients_rejected() {
        assert!(
            build(&Outgoing {
                from: "a@b.c",
                to: &[],
                cc: &[],
                subject: "",
                body: "",
                in_reply_to: None,
                references: &[],
                parts: &[],
                links: &[],
            })
            .is_err()
        );
    }

    #[rstest]
    fn one_file_is_multipart_mixed(one_file: Vec<u8>) {
        assert!(String::from_utf8_lossy(&one_file).contains("Content-Type: multipart/mixed"));
    }

    #[rstest]
    fn one_file_keeps_its_bytes(one_file: Vec<u8>) {
        assert_eq!(parse(&one_file).attachment(0).unwrap().contents(), PDF);
    }

    #[rstest]
    fn one_file_keeps_the_text(one_file: Vec<u8>) {
        assert_eq!(parse(&one_file).body_text(0).unwrap().trim(), "Looks good");
    }

    #[rstest]
    fn one_file_is_base64(one_file: Vec<u8>) {
        assert!(String::from_utf8_lossy(&one_file).contains("Content-Transfer-Encoding: base64"));
    }

    #[rstest]
    fn one_file_has_its_name(one_file: Vec<u8>) {
        assert_eq!(
            parse(&one_file).attachment(0).unwrap().attachment_name(),
            Some("report.pdf")
        );
    }

    #[rstest]
    fn several_files_are_all_attached(two_files: Vec<u8>) {
        assert_eq!(parse(&two_files).attachment_count(), 2);
    }

    #[rstest]
    fn second_file_keeps_its_bytes(two_files: Vec<u8>) {
        assert_eq!(parse(&two_files).attachment(1).unwrap().contents(), PNG);
    }

    #[rstest]
    fn second_file_has_its_type(two_files: Vec<u8>) {
        assert_eq!(
            parse(&two_files)
                .attachment(1)
                .unwrap()
                .content_type()
                .unwrap()
                .subtype(),
            Some("png")
        );
    }

    #[rstest]
    fn inline_part_is_inline(two_files: Vec<u8>) {
        assert!(String::from_utf8_lossy(&two_files).contains("Content-Disposition: inline"));
    }

    #[rstest]
    fn attached_part_is_an_attachment(two_files: Vec<u8>) {
        assert!(String::from_utf8_lossy(&two_files).contains("Content-Disposition: attachment"));
    }

    #[rstest]
    #[case::accents("Relevé d'été.pdf")]
    #[case::spaces("budget final v2.pdf")]
    #[case::cjk("報告書.pdf")]
    fn non_ascii_filename_round_trips(#[case] name: &str) {
        let raw = build_with(
            &[Part {
                filename: name,
                mime: "application/pdf",
                bytes: PDF,
                inline: false,
            }],
            &[],
            false,
        );
        assert_eq!(
            parse(&raw).attachment(0).unwrap().attachment_name(),
            Some(name)
        );
    }

    #[rstest]
    fn non_ascii_filename_header_stays_ascii() {
        let raw = build_with(
            &[Part {
                filename: "Relevé d'été.pdf",
                mime: "application/pdf",
                bytes: PDF,
                inline: false,
            }],
            &[],
            false,
        );
        assert!(raw.is_ascii());
    }

    #[rstest]
    fn reply_with_file_keeps_in_reply_to(reply_with_file: String) {
        assert!(reply_with_file.contains("In-Reply-To: <a@x>"));
    }

    #[rstest]
    fn reply_with_file_keeps_references(reply_with_file: String) {
        assert!(reply_with_file.contains("References: <a@x>"));
    }

    #[rstest]
    fn links_are_listed_under_the_text() {
        let raw = build_with(
            &[],
            &[Link {
                title: "Plan.pdf",
                url: "https://drive.example/f/1",
            }],
            false,
        );
        assert_eq!(
            parse(&raw)
                .body_text(0)
                .unwrap()
                .replace("\r\n", "\n")
                .trim(),
            "Looks good\n\nPlan.pdf: https://drive.example/f/1"
        );
    }

    #[rstest]
    #[case::empty(0, true)]
    #[case::small(1024, true)]
    #[case::at_limit(MAX_ATTACHMENT_BYTES, true)]
    #[case::one_byte_over(MAX_ATTACHMENT_BYTES + 1, false)]
    #[case::far_over(40 * 1024 * 1024, false)]
    fn size_limit(#[case] total: u64, #[case] ok: bool) {
        assert_eq!(check_total(total).is_ok(), ok);
    }

    #[rstest]
    fn size_limit_message_is_plain_words() {
        let msg = check_total(30 * 1024 * 1024).unwrap_err().to_string();
        assert!(
            msg.contains("These files add up to 30.0 MB, more than the 25.0 MB Gmail can send")
        );
    }

    #[rstest]
    fn build_refuses_files_over_the_limit() {
        let big = vec![0u8; MAX_ATTACHMENT_BYTES as usize + 1];
        let to = to();
        let r = build(&Outgoing {
            from: "me@example.com",
            to: &to,
            cc: &[],
            subject: "Big",
            body: "",
            in_reply_to: None,
            references: &[],
            parts: &[Part {
                filename: "big.bin",
                mime: "application/octet-stream",
                bytes: &big,
                inline: false,
            }],
            links: &[],
        });
        assert!(r.is_err());
    }

    #[rstest]
    #[case::pdf_ext("a.PDF", b"", "application/pdf")]
    #[case::docx(
        "notes.docx",
        b"PK\x03\x04",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    )]
    #[case::sniff_png("pasted", b"\x89PNG\r\n\x1a\n....", "image/png")]
    #[case::sniff_pdf("scan", b"%PDF-1.7", "application/pdf")]
    #[case::sniff_jpeg("photo", b"\xFF\xD8\xFF\xE0", "image/jpeg")]
    #[case::unknown("blob.xyz", b"\x00\x01", "application/octet-stream")]
    fn content_type_detection(#[case] name: &str, #[case] head: &[u8], #[case] want: &str) {
        assert_eq!(mime_for(name, head), want);
    }

    #[rstest]
    fn quote_names_the_author_and_date() {
        let from = crate::model::Address {
            name: "Franck ADAM".into(),
            email: "franck@plomberie.example".into(),
        };
        assert_eq!(
            quote_block(
                "Merci.",
                "Tue, Oct 13, 2026 at 01:41",
                &from,
                "Votre facture.\n\nCordialement"
            ),
            "Merci.\n\nOn Tue, Oct 13, 2026 at 01:41, Franck ADAM <franck@plomberie.example> wrote:\n> Votre facture.\n>\n> Cordialement\n"
        );
    }

    #[rstest]
    #[case::kb(1536, "2 KB")]
    #[case::mb(3 * 1024 * 1024 + 400 * 1024, "3.4 MB")]
    fn human_sizes(#[case] bytes: u64, #[case] want: &str) {
        assert_eq!(human_size(bytes), want);
    }
}
