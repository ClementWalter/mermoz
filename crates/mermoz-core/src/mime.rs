//! RFC 5322 parsing of raw provider messages into what the store keeps.

use crate::model::Address;
use mail_parser::{MessageParser, MimeHeaders};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedAttachment {
    pub filename: String,
    pub mime: String,
    pub data: Vec<u8>,
    /// `Content-ID` without angle brackets: what `cid:` links in the HTML point to.
    pub content_id: Option<String>,
    /// Shown inside the body (an image pasted in the text), not a file sent along.
    pub inline: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedMessage {
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    /// Only present on the sender's own copy of a message.
    pub bcc: Vec<Address>,
    /// Where replies go (`Reply-To`), e.g. the person behind a no-reply sender.
    pub reply_to: Vec<Address>,
    pub subject: String,
    pub date: Option<i64>,
    pub body_text: String,
    pub body_html: Option<String>,
    pub message_id: Option<String>,
    pub references: Vec<String>,
    pub attachments: Vec<ParsedAttachment>,
    /// `Authentication-Results` header values, topmost first (see `auth`).
    pub auth_results: Vec<String>,
    /// The mailing list's identifier (`List-Id`), lowercase, without its display name.
    pub list_id: Option<String>,
}

fn addrs(a: Option<&mail_parser::Address>) -> Vec<Address> {
    let Some(a) = a else { return vec![] };
    a.iter()
        .filter_map(|x| {
            let email = x.address.as_deref()?.trim().to_string();
            if email.is_empty() {
                return None;
            }
            Some(Address {
                name: x.name.as_deref().unwrap_or("").trim().to_string(),
                email,
            })
        })
        .collect()
}

/// Parses raw bytes; returns `None` only for input that isn't a message at all.
pub fn parse(raw: &[u8]) -> Option<ParsedMessage> {
    let msg = MessageParser::default().parse(raw)?;
    // body_html() synthesizes HTML from plain text; only keep it when a real HTML part exists.
    let has_html = msg
        .parts
        .iter()
        .any(|p| matches!(p.body, mail_parser::PartType::Html(_)));
    let body_html = if has_html {
        msg.body_html(0).map(|s| s.into_owned())
    } else {
        None
    };
    let attachments = msg
        .attachments()
        .map(|p| {
            let mime = p
                .content_type()
                .map(|ct| match ct.subtype() {
                    Some(sub) => format!("{}/{}", ct.ctype(), sub),
                    None => ct.ctype().to_string(),
                })
                .unwrap_or_else(|| "application/octet-stream".into());
            let content_id = p
                .content_id()
                .map(|c| {
                    c.trim()
                        .trim_start_matches('<')
                        .trim_end_matches('>')
                        .to_string()
                })
                .filter(|c| !c.is_empty());
            // Inline: placed in the body (pasted image). An explicit "attachment"
            // disposition is a file sent along, even when it carries a Content-ID.
            let disposition = p
                .content_disposition()
                .map(|d| d.ctype().to_ascii_lowercase());
            let inline = match disposition.as_deref() {
                Some("attachment") => false,
                Some("inline") => true,
                _ => content_id.is_some(),
            };
            ParsedAttachment {
                filename: p.attachment_name().unwrap_or("attachment").to_string(),
                mime,
                data: p.contents().to_vec(),
                content_id,
                inline,
            }
        })
        .collect();
    let mut references: Vec<String> = msg
        .references()
        .as_text_list()
        .map(|v| v.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    if references.is_empty()
        && let Some(r) = msg.in_reply_to().as_text()
    {
        references.push(r.to_string());
    }
    Some(ParsedMessage {
        from: addrs(msg.from()).into_iter().next(),
        to: addrs(msg.to()),
        cc: addrs(msg.cc()),
        bcc: addrs(msg.bcc()),
        reply_to: addrs(msg.reply_to()),
        subject: msg.subject().unwrap_or("").to_string(),
        date: msg.date().map(|d| d.to_timestamp() * 1000),
        body_text: msg.body_text(0).map(|s| s.into_owned()).unwrap_or_default(),
        body_html,
        message_id: msg.message_id().map(|s| s.to_string()),
        references,
        attachments,
        auth_results: crate::auth::results_from_raw(raw),
        list_id: msg.header_raw("List-Id").and_then(list_id),
    })
}

/// Addresses in one address-list header value, e.g. a `Reply-To` read on its own.
pub fn header_addresses(value: &str) -> Vec<Address> {
    let raw = format!("Reply-To: {}\r\n\r\n", value.trim());
    MessageParser::default()
        .parse(raw.as_bytes())
        .map(|m| addrs(m.reply_to()))
        .unwrap_or_default()
}

/// "Runners <runners.club.example>" → "runners.club.example".
fn list_id(raw: &str) -> Option<String> {
    let v = raw.trim();
    let id = match (v.rfind('<'), v.rfind('>')) {
        (Some(a), Some(b)) if a < b => &v[a + 1..b],
        _ => v,
    };
    let id = id.trim().to_lowercase();
    (!id.is_empty()).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::*;

    const SIMPLE: &[u8] = b"From: Marc Petit <marc@example.com>\r\nTo: You <me@example.com>\r\nCc: jonas@example.com\r\nSubject: Audit v2\r\nDate: Sun, 11 Oct 2026 08:12:00 +0000\r\nMessage-ID: <abc@example.com>\r\n\r\nHello there\r\n";

    const MULTIPART: &[u8] = b"From: a@example.com\r\nTo: b@example.com\r\nSubject: Files\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"XX\"\r\n\r\n--XX\r\nContent-Type: text/html\r\n\r\n<p>Hi <b>there</b></p>\r\n--XX\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"r.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0=\r\n--XX--\r\n";

    #[fixture]
    fn simple() -> ParsedMessage {
        parse(SIMPLE).unwrap()
    }

    #[fixture]
    fn multipart() -> ParsedMessage {
        parse(MULTIPART).unwrap()
    }

    #[rstest]
    fn from_name_is_parsed(simple: ParsedMessage) {
        assert_eq!(simple.from.unwrap().name, "Marc Petit");
    }

    #[rstest]
    fn cc_without_name_is_parsed(simple: ParsedMessage) {
        assert_eq!(simple.cc[0].email, "jonas@example.com");
    }

    #[rstest]
    fn date_is_unix_ms(simple: ParsedMessage) {
        assert_eq!(simple.date, Some(1_791_706_320_000));
    }

    #[rstest]
    fn plain_body_has_no_html(simple: ParsedMessage) {
        assert_eq!(simple.body_html, None);
    }

    #[rstest]
    fn html_only_body_yields_text(multipart: ParsedMessage) {
        assert!(multipart.body_text.contains("Hi there"));
    }

    #[rstest]
    fn attachment_mime_is_parsed(multipart: ParsedMessage) {
        assert_eq!(multipart.attachments[0].mime, "application/pdf");
    }

    #[rstest]
    fn attachment_bytes_are_decoded(multipart: ParsedMessage) {
        assert_eq!(multipart.attachments[0].data, b"%PDF-");
    }

    #[rstest]
    fn reply_to_is_read() {
        let raw = b"From: Franck ADAM <no-reply@invoices.example>\r\nReply-To: Franck ADAM <franck@plomberie.example>\r\nTo: me@example.com\r\nSubject: s\r\n\r\nhi\r\n";
        assert_eq!(
            parse(raw).unwrap().reply_to[0].email,
            "franck@plomberie.example"
        );
    }

    #[rstest]
    fn header_value_yields_its_address() {
        assert_eq!(
            header_addresses("\"Franck ADAM\" <franck@plomberie.example>")[0].email,
            "franck@plomberie.example"
        );
    }

    #[rstest]
    fn missing_reply_to_is_empty(simple: ParsedMessage) {
        assert!(simple.reply_to.is_empty());
    }

    #[rstest]
    fn blind_copies_are_read() {
        let raw = b"From: me@example.com\r\nTo: a@x.com\r\nBcc: Boss <boss@x.com>\r\nSubject: s\r\n\r\nhi\r\n";
        assert_eq!(parse(raw).unwrap().bcc[0].email, "boss@x.com");
    }

    #[rstest]
    #[case::named("Runners <Runners.Club.example>", Some("runners.club.example"))]
    #[case::bare("announce.club.example", Some("announce.club.example"))]
    #[case::empty("  ", None)]
    fn list_ids_keep_the_identifier(#[case] raw: &str, #[case] want: Option<&str>) {
        assert_eq!(list_id(raw).as_deref(), want);
    }

    #[rstest]
    fn list_id_header_is_read() {
        let raw = b"From: ada@club.example\r\nTo: runners@club.example\r\nList-Id: Runners <runners.club.example>\r\nSubject: s\r\n\r\nhi\r\n";
        assert_eq!(
            parse(raw).unwrap().list_id.as_deref(),
            Some("runners.club.example")
        );
    }
}
