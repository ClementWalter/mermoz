//! Machine mail in a conversation: delivery failure reports (bounces, RFC 3464) and
//! auto-replies (RFC 3834, out-of-office). A bounce is read for who it failed for, why,
//! and which sent message it reports on, so that message can carry the failure.

use mail_parser::{MessageParser, MimeHeaders, PartType};
use serde::{Deserialize, Serialize};

/// What kind of sender wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoKind {
    Human,
    /// A delivery status notification: the mail system reporting a failed send.
    Bounce,
    /// An automatic answer (out of office, "we received your request").
    AutoReply,
}

impl AutoKind {
    /// How the store keeps it.
    pub fn as_str(self) -> &'static str {
        match self {
            AutoKind::Human => "",
            AutoKind::Bounce => "bounce",
            AutoKind::AutoReply => "auto_reply",
        }
    }

    pub fn from_stored(s: &str) -> Self {
        match s {
            "bounce" => AutoKind::Bounce,
            "auto_reply" => AutoKind::AutoReply,
            _ => AutoKind::Human,
        }
    }
}

/// What a bounce says about one failed recipient.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Dsn {
    pub recipient: String,
    /// RFC 3463 status code, e.g. "5.1.1".
    pub status: String,
    /// The receiving server's own words (`Diagnostic-Code`), without the "smtp;" type.
    pub diagnostic: String,
    /// `Message-ID` of the message that failed, without angle brackets.
    pub original_message_id: Option<String>,
}

/// Local parts (separators removed) of the mail system's own senders.
const BOUNCE_SENDERS: &[&str] = &["mailerdaemon", "postmaster"];

/// Subject openings of automatic answers, lowercase.
const AUTO_SUBJECTS: &[&str] = &[
    "out of office",
    "automatic reply",
    "auto reply",
    "auto-reply",
    "autoreply",
    "réponse automatique",
    "reponse automatique",
    "absence du bureau",
    "absent du bureau",
];

fn squashed_local(email: &str) -> String {
    let local = email.trim().rsplit_once('@').map_or("", |(l, _)| l);
    local
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | '.'))
        .collect()
}

/// Whether the address is the mail system reporting on delivery.
pub fn is_bounce_sender(email: &str) -> bool {
    BOUNCE_SENDERS.contains(&squashed_local(email).as_str())
}

/// Whether a subject announces an automatic answer.
pub fn is_auto_subject(subject: &str) -> bool {
    let s = subject.trim().to_lowercase();
    AUTO_SUBJECTS.iter().any(|a| s.contains(a))
}

/// The kind of a message from its sender, subject and headers (`header(name)` gives a
/// header's value, any case of the name).
pub fn classify(from: &str, subject: &str, header: impl Fn(&str) -> Option<String>) -> AutoKind {
    let ct = header("Content-Type").unwrap_or_default().to_lowercase();
    if (ct.contains("multipart/report") && ct.contains("delivery-status")) || is_bounce_sender(from)
    {
        return AutoKind::Bounce;
    }
    let submitted = header("Auto-Submitted").unwrap_or_default();
    let submitted = submitted.trim().to_lowercase();
    if (!submitted.is_empty() && submitted != "no")
        || header("X-Autoreply").is_some()
        || header("X-Autorespond").is_some()
        || is_auto_subject(subject)
    {
        return AutoKind::AutoReply;
    }
    AutoKind::Human
}

/// A message's kind from its raw bytes.
pub fn classify_raw(raw: &[u8], from: &str, subject: &str) -> AutoKind {
    let Some(msg) = MessageParser::default().parse(raw) else {
        return classify(from, subject, |_| None);
    };
    classify(from, subject, |name| {
        msg.header_raw(name).map(|v| v.trim().to_string())
    })
}

/// Joins folded header lines ("Diagnostic-Code: smtp; 550 …\n  continued").
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = vec![];
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with([' ', '\t'])
            && let Some(last) = lines.last_mut()
        {
            last.push(' ');
            last.push_str(line.trim());
            continue;
        }
        lines.push(line.to_string());
    }
    lines
}

/// The value of `name:` in header-style text, first occurrence.
fn field(lines: &[String], name: &str) -> Option<String> {
    lines.iter().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim().to_string())
    })
}

/// "rfc822; a@b" → "a@b"; "smtp; 550 …" → "550 …".
fn after_type(v: &str) -> String {
    match v.split_once(';') {
        Some((_, rest)) => rest.trim().to_string(),
        None => v.trim().to_string(),
    }
}

fn bare_id(v: &str) -> String {
    v.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

/// Reads a delivery status notification; `None` when the bytes aren't one.
pub fn parse(raw: &[u8]) -> Option<Dsn> {
    let msg = MessageParser::default().parse(raw)?;
    let mut status_text: Option<String> = None;
    let mut original: Option<String> = None;
    for part in &msg.parts {
        let ct = part.content_type();
        let (ty, sub) = ct.map_or((String::new(), String::new()), |c| {
            (
                c.ctype().to_lowercase(),
                c.subtype().unwrap_or_default().to_lowercase(),
            )
        });
        match (&part.body, ty.as_str(), sub.as_str()) {
            (PartType::Message(inner), _, _) => {
                original = original.or_else(|| inner.message_id().map(bare_id));
            }
            (_, "message", "delivery-status") => {
                status_text = Some(String::from_utf8_lossy(part.contents()).into_owned());
            }
            (_, "text", "rfc822-headers") | (_, "message", "rfc822-headers") => {
                let text = String::from_utf8_lossy(part.contents()).into_owned();
                original =
                    original.or_else(|| field(&unfold(&text), "Message-ID").map(|v| bare_id(&v)));
            }
            _ => {}
        }
    }
    let lines = unfold(&status_text?);
    let recipient = field(&lines, "Final-Recipient")
        .or_else(|| field(&lines, "Original-Recipient"))
        .map(|v| after_type(&v))?;
    original = original.or_else(|| field(&lines, "X-Original-Message-ID").map(|v| bare_id(&v)));
    Some(Dsn {
        recipient,
        status: field(&lines, "Status").unwrap_or_default(),
        diagnostic: field(&lines, "Diagnostic-Code")
            .map(|v| after_type(&v))
            .unwrap_or_default(),
        original_message_id: original,
    })
}

/// The reason in plain words, from the status code (else the server's words).
pub fn reason(dsn: &Dsn) -> String {
    let s = dsn.status.trim();
    let known = match s {
        "5.1.1" | "5.1.0" | "5.1.10" | "5.1.6" => Some("address doesn't exist"),
        "5.1.2" | "5.4.4" => Some("the domain doesn't exist"),
        "5.2.2" | "4.2.2" => Some("the mailbox is full"),
        "5.2.1" => Some("the mailbox is disabled"),
        "5.3.4" | "5.2.3" => Some("the message is too large"),
        _ => None,
    };
    if let Some(k) = known {
        return k.to_string();
    }
    if s.starts_with("5.7") {
        return "the receiving server refused it".into();
    }
    if s.starts_with('4') {
        return "delivery is delayed, still trying".into();
    }
    if !dsn.diagnostic.is_empty() {
        return dsn.diagnostic.clone();
    }
    "it couldn't be delivered".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const GMAIL_EN: &[u8] = include_bytes!("dsn/gmail_en.eml");
    const GMAIL_FR: &[u8] = include_bytes!("dsn/gmail_fr.eml");

    fn header_map(
        pairs: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.to_string())
        }
    }

    #[rstest]
    fn english_bounce_names_the_recipient() {
        assert_eq!(
            parse(GMAIL_EN).unwrap().recipient,
            "no-reply@invoices.example"
        );
    }

    #[rstest]
    fn english_bounce_has_its_status() {
        assert_eq!(parse(GMAIL_EN).unwrap().status, "5.1.1");
    }

    #[rstest]
    fn english_bounce_unfolds_the_diagnostic() {
        assert_eq!(
            parse(GMAIL_EN).unwrap().diagnostic,
            "550-5.1.1 The email account that you tried to reach does not exist. 550 5.1.1 fictional-id - gsmtp"
        );
    }

    #[rstest]
    fn english_bounce_names_the_failed_message() {
        assert_eq!(
            parse(GMAIL_EN).unwrap().original_message_id.as_deref(),
            Some("sent-en@mail.gmail.example")
        );
    }

    #[rstest]
    fn french_bounce_names_the_recipient() {
        assert_eq!(
            parse(GMAIL_FR).unwrap().recipient,
            "contact@ancien-domaine.example"
        );
    }

    #[rstest]
    fn french_bounce_has_its_status() {
        assert_eq!(parse(GMAIL_FR).unwrap().status, "5.1.2");
    }

    #[rstest]
    fn french_bounce_reads_the_failed_message_from_its_headers() {
        assert_eq!(
            parse(GMAIL_FR).unwrap().original_message_id.as_deref(),
            Some("sent-fr@mail.gmail.example")
        );
    }

    #[rstest]
    fn ordinary_mail_is_not_a_report() {
        let raw = b"From: a@example.com\r\nTo: b@example.com\r\nSubject: hi\r\n\r\nhello\r\n";
        assert_eq!(parse(raw), None);
    }

    #[rstest]
    #[case::exists("5.1.1", "address doesn't exist")]
    #[case::domain("5.1.2", "the domain doesn't exist")]
    #[case::full("5.2.2", "the mailbox is full")]
    #[case::policy("5.7.1", "the receiving server refused it")]
    #[case::delayed("4.4.1", "delivery is delayed, still trying")]
    fn reason_in_plain_words(#[case] status: &str, #[case] want: &str) {
        let dsn = Dsn {
            status: status.into(),
            ..Default::default()
        };
        assert_eq!(reason(&dsn), want);
    }

    #[rstest]
    fn unknown_status_gives_the_servers_words() {
        let dsn = Dsn {
            status: "5.9.9".into(),
            diagnostic: "550 rejected".into(),
            ..Default::default()
        };
        assert_eq!(reason(&dsn), "550 rejected");
    }

    #[rstest]
    fn english_gmail_bounce_is_a_bounce() {
        assert_eq!(classify_raw(GMAIL_EN, "", ""), AutoKind::Bounce);
    }

    #[rstest]
    fn french_gmail_bounce_is_a_bounce() {
        assert_eq!(classify_raw(GMAIL_FR, "", ""), AutoKind::Bounce);
    }

    #[rstest]
    #[case::mailer_daemon("MAILER-DAEMON@mx.example")]
    #[case::gmail("mailer-daemon@googlemail.com")]
    #[case::postmaster("postmaster@outlook.example")]
    fn mail_system_sender_is_a_bounce(#[case] from: &str) {
        assert_eq!(classify(from, "Undeliverable", |_| None), AutoKind::Bounce);
    }

    #[rstest]
    fn report_content_type_is_a_bounce() {
        let h = header_map(&[(
            "Content-Type",
            "multipart/report; report-type=delivery-status; boundary=x",
        )]);
        assert_eq!(
            classify("relay@isp.example", "Returned mail", h),
            AutoKind::Bounce
        );
    }

    #[rstest]
    #[case::auto_replied(&[("Auto-Submitted", "auto-replied")])]
    #[case::auto_generated(&[("Auto-Submitted", "auto-generated")])]
    #[case::x_autoreply(&[("X-Autoreply", "yes")])]
    #[case::x_autorespond(&[("X-Autorespond", "on")])]
    fn auto_reply_headers_mark_an_auto_reply(#[case] h: &'static [(&'static str, &'static str)]) {
        assert_eq!(
            classify("lea@acme.example", "Re: Devis", header_map(h)),
            AutoKind::AutoReply
        );
    }

    #[rstest]
    fn auto_submitted_no_is_a_person() {
        let h = header_map(&[("Auto-Submitted", "no")]);
        assert_eq!(
            classify("lea@acme.example", "Re: Devis", h),
            AutoKind::Human
        );
    }

    #[rstest]
    #[case::english("Out of Office: back on Monday")]
    #[case::outlook("Automatic reply: Devis")]
    #[case::french("Réponse automatique : Devis")]
    #[case::french_absence("Absence du bureau")]
    fn auto_reply_subject_marks_an_auto_reply(#[case] subject: &str) {
        assert_eq!(
            classify("lea@acme.example", subject, |_| None),
            AutoKind::AutoReply
        );
    }

    #[rstest]
    fn a_person_writing_is_human() {
        assert_eq!(
            classify("lea@acme.example", "Re: Devis", |_| None),
            AutoKind::Human
        );
    }
}
