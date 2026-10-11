//! Outgoing mail: builds the RFC 5322 bytes for a draft, threaded when replying.

use crate::error::{Error, Result};
use mail_builder::MessageBuilder;

pub struct Outgoing<'a> {
    pub from: &'a str,
    pub to: &'a [String],
    pub cc: &'a [String],
    pub subject: &'a str,
    pub body: &'a str,
    pub in_reply_to: Option<&'a str>,
    pub references: &'a [String],
}

fn strip_brackets(s: &str) -> String {
    s.trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_string()
}

/// Plain-text message; reply headers keep it in the recipient's existing thread.
pub fn build(o: &Outgoing) -> Result<Vec<u8>> {
    if o.to.is_empty() && o.cc.is_empty() {
        return Err(Error::InvalidParams("draft has no recipients".into()));
    }
    let mut b = MessageBuilder::new()
        .from(o.from.to_string())
        .to(o.to.iter().map(|s| s.as_str()).collect::<Vec<_>>())
        .subject(o.subject)
        .text_body(o.body);
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
    b.write_to_vec().map_err(|e| Error::other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn sample() -> String {
        let to = vec!["marc@example.com".to_string()];
        let refs = vec!["<a@x>".to_string()];
        let raw = build(&Outgoing {
            from: "me@example.com",
            to: &to,
            cc: &[],
            subject: "Re: Audit",
            body: "Looks good",
            in_reply_to: Some("<a@x>"),
            references: &refs,
        })
        .unwrap();
        String::from_utf8(raw).unwrap()
    }

    #[rstest]
    fn has_in_reply_to() {
        assert!(sample().contains("In-Reply-To: <a@x>"));
    }

    #[rstest]
    fn has_body() {
        assert!(sample().contains("Looks good"));
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
                references: &[]
            })
            .is_err()
        );
    }
}
