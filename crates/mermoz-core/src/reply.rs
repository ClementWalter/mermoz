//! Who a reply goes to (RFC 5322 §3.6.3): `Reply-To` when the sender set one, else
//! `From`; reply-all adds the other recipients minus the user. The message answered
//! by default is the latest from a real correspondent, never a bounce or an
//! auto-reply. Flags addresses that don't accept replies and, when it can, names the
//! person behind them.

use crate::model::{Address, Message};
use serde::{Deserialize, Serialize};

/// The recipients a reply to a conversation starts with.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ReplyAddressing {
    /// The message the reply answers (threads onto it, quotes it).
    pub message_id: String,
    /// Its Gmail thread, which the reply continues.
    pub thread_id: String,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    /// Addresses in `to` that don't accept replies: the composer warns about them.
    pub no_reply: Vec<String>,
    /// A person's address found in the mail, offered in place of a no-reply `to`.
    pub suggestion: Option<Address>,
}

/// Local parts (separators removed) that mark an address nobody reads.
const NO_REPLY_MARKS: &[&str] = &["noreply", "donotreply", "nepasrepondre", "nepasrépondre"];

/// Whole local parts that are robots, not people.
const ROBOT_LOCALS: &[&str] = &["notifications", "notification", "mailerdaemon"];

/// Whether mail sent to `email` reaches nobody (no-reply, notifications, bounces).
pub fn is_no_reply(email: &str) -> bool {
    let Some((local, _)) = email.trim().rsplit_once('@') else {
        return false;
    };
    let squashed: String = local
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | '.' | '+'))
        .collect();
    NO_REPLY_MARKS.iter().any(|m| squashed.contains(m)) || ROBOT_LOCALS.contains(&squashed.as_str())
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Where a received message's replies go: its `Reply-To`, else its sender.
pub fn reply_target(m: &Message) -> Vec<Address> {
    if m.reply_to.is_empty() {
        vec![m.from.clone()]
    } else {
        m.reply_to.clone()
    }
}

/// Appends the addresses not already present (case-insensitive) and not the user's.
fn add_unique(into: &mut Vec<Address>, from: &[Address], skip: &[Address], me: &[String]) {
    for a in from {
        let taken = into.iter().chain(skip).any(|x| same(&x.email, &a.email));
        let mine = me.iter().any(|m| same(m, &a.email));
        if !taken && !mine && !a.email.trim().is_empty() {
            into.push(a.clone());
        }
    }
}

/// Reply-all to `last`. On a received message: To is its reply target plus its To,
/// Cc its Cc, both without the user. On the user's own message: the same recipients
/// again, with any sender that asked for replies elsewhere (`redirects`) swapped for
/// that address.
pub fn reply_all(
    last: &Message,
    me: &[String],
    redirects: &[Message],
) -> (Vec<Address>, Vec<Address>) {
    let mut to = vec![];
    let mut cc = vec![];
    if last.is_from_me {
        let swap = |a: &Address| -> Vec<Address> {
            redirects
                .iter()
                .rev()
                .find(|m| !m.is_from_me && !m.reply_to.is_empty() && same(&m.from.email, &a.email))
                .map(|m| m.reply_to.clone())
                .unwrap_or_else(|| vec![a.clone()])
        };
        let swapped: Vec<Address> = last.to.iter().flat_map(swap).collect();
        add_unique(&mut to, &swapped, &[], me);
        add_unique(&mut cc, &last.cc, &to, me);
    } else {
        add_unique(&mut to, &reply_target(last), &[], me);
        add_unique(&mut to, &last.to, &[], me);
        add_unique(&mut cc, &last.cc, &to, me);
    }
    (to, cc)
}

/// Characters around an address in running text.
fn is_edge(c: char) -> bool {
    c.is_whitespace() || "<>()[]{},;:\"'«»|".contains(c)
}

fn looks_like_email(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

/// Addresses written in a text (signature, "contact us at …"), in order, deduplicated.
pub fn emails_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for token in text.split(is_edge) {
        let t = token
            .trim_start_matches("mailto")
            .trim_matches(|c: char| matches!(c, '.' | '!' | '?' | '*'))
            .to_lowercase();
        if looks_like_email(&t) && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// The display name without a platform's "(via Tolteck)" style suffix.
fn person_name(name: &str) -> String {
    match name.find(" (via ") {
        Some(i) => name[..i].trim().to_string(),
        None => name.trim().to_string(),
    }
}

/// A person's address for a message sent from a no-reply address: one in its text
/// whose local part carries a word of the sender's name, else the first one found.
pub fn suggest_human(m: &Message, me: &[String]) -> Option<Address> {
    let found: Vec<String> = emails_in(&m.body_text)
        .into_iter()
        .filter(|e| !is_no_reply(e) && !me.iter().any(|x| same(x, e)))
        .collect();
    let name = person_name(&m.from.name);
    let words: Vec<String> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(|w| w.to_lowercase())
        .collect();
    let by_name = found.iter().find(|e| {
        let local = e.split('@').next().unwrap_or_default();
        words.iter().any(|w| local.contains(w.as_str()))
    });
    match by_name {
        Some(e) => Some(Address {
            name,
            email: e.clone(),
        }),
        None => found.into_iter().next().map(|email| Address {
            name: String::new(),
            email,
        }),
    }
}

/// A message from a person the user can answer: not the user, not the mail system or
/// an auto-reply, and not a no-reply sender without a Reply-To that reaches someone.
pub fn is_real_correspondent(m: &Message) -> bool {
    let reachable =
        !is_no_reply(&m.from.email) || m.reply_to.iter().any(|a| !is_no_reply(&a.email));
    !m.is_from_me && !m.is_bounce && !m.is_auto_reply && reachable
}

/// The message a reply answers by default (`messages` oldest first): the latest from a
/// real correspondent; else the user's latest (a follow-up keeps its recipients); else
/// the latest that isn't a bounce or auto-reply; else the latest.
pub fn target(messages: &[Message]) -> Option<&Message> {
    let latest = |f: &dyn Fn(&Message) -> bool| messages.iter().rev().find(|m| f(m));
    latest(&is_real_correspondent)
        .or_else(|| latest(&|m| m.is_from_me))
        .or_else(|| latest(&|m| !m.is_bounce && !m.is_auto_reply))
        .or_else(|| messages.last())
}

/// The reply-all to `answered`, with its no-reply warning and suggestion.
pub fn addressing_for(answered: &Message, messages: &[Message], me: &[String]) -> ReplyAddressing {
    let (to, cc) = reply_all(answered, me, messages);
    let no_reply: Vec<String> = to
        .iter()
        .filter(|a| is_no_reply(&a.email))
        .map(|a| a.email.clone())
        .collect();
    let suggestion = if no_reply.is_empty() {
        None
    } else {
        messages
            .iter()
            .rev()
            .filter(|m| !m.is_from_me && !m.is_bounce)
            .find_map(|m| suggest_human(m, me))
    };
    ReplyAddressing {
        message_id: answered.id.clone(),
        thread_id: answered.thread_id.clone(),
        to,
        cc,
        no_reply,
        suggestion,
    }
}

/// The reply-all a conversation starts with (`messages` oldest first), answering its
/// [`target`]. `None` for a conversation without messages.
pub fn addressing(messages: &[Message], me: &[String]) -> Option<ReplyAddressing> {
    Some(addressing_for(target(messages)?, messages, me))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::{fixture, rstest};

    const ME: &str = "me@example.com";

    fn addr(email: &str) -> Address {
        Address {
            name: String::new(),
            email: email.into(),
        }
    }

    fn msg(from: &str, to: &[&str], cc: &[&str], reply_to: &[&str]) -> Message {
        Message {
            id: "m".into(),
            thread_id: "t".into(),
            from: addr(from),
            to: to.iter().map(|e| addr(e)).collect(),
            cc: cc.iter().map(|e| addr(e)).collect(),
            bcc: vec![],
            reply_to: reply_to.iter().map(|e| addr(e)).collect(),
            is_bounce: false,
            is_auto_reply: false,
            delivery_failure: None,
            date: 0,
            subject: "s".into(),
            body_text: String::new(),
            body_new: String::new(),
            has_quoted: false,
            has_signature: false,
            forwarded: false,
            forwarded_from: None,
            quoted_only: false,
            reaction: None,
            body_clean: None,
            has_html: false,
            is_from_me: from == ME,
            attachments: vec![],
            auth: None,
        }
    }

    fn me() -> Vec<String> {
        vec![ME.to_string()]
    }

    fn emails(v: &[Address]) -> Vec<&str> {
        v.iter().map(|a| a.email.as_str()).collect()
    }

    /// An invoice sent by a platform on behalf of a craftsman.
    #[fixture]
    fn invoice() -> Message {
        let mut m = msg(
            "no-reply@invoices.example",
            &[ME],
            &[],
            &["franck@plomberie.example"],
        );
        m.from.name = "Franck ADAM (via Invoices)".into();
        m
    }

    #[fixture]
    fn invoice_without_reply_to() -> Message {
        let mut m = msg("no-reply@invoices.example", &[ME], &[], &[]);
        m.from.name = "Franck ADAM (via Invoices)".into();
        m.body_text = "Bonjour,\nVotre facture est jointe.\n\nFranck ADAM\nPlomberie Adam\ncontact: compta@plomberie.example — franck.adam@plomberie.example\nNe pas répondre: no-reply@invoices.example\n".into();
        m
    }

    #[rstest]
    fn reply_goes_to_reply_to(invoice: Message) {
        assert_eq!(
            emails(&reply_all(&invoice, &me(), &[]).0),
            vec!["franck@plomberie.example"]
        );
    }

    #[rstest]
    fn reply_goes_to_from_without_reply_to() {
        let m = msg("marc@acme.example", &[ME], &[], &[]);
        assert_eq!(
            emails(&reply_all(&m, &me(), &[]).0),
            vec!["marc@acme.example"]
        );
    }

    #[rstest]
    fn reply_to_with_several_addresses_keeps_all() {
        let m = msg(
            "bot@acme.example",
            &[ME],
            &[],
            &["a@acme.example", "b@acme.example"],
        );
        assert_eq!(
            emails(&reply_all(&m, &me(), &[]).0),
            vec!["a@acme.example", "b@acme.example"]
        );
    }

    #[rstest]
    fn reply_all_adds_the_other_to_recipients() {
        let m = msg("marc@acme.example", &[ME, "lea@acme.example"], &[], &[]);
        assert_eq!(
            emails(&reply_all(&m, &me(), &[]).0),
            vec!["marc@acme.example", "lea@acme.example"]
        );
    }

    #[rstest]
    fn reply_all_keeps_cc_in_cc() {
        let m = msg("marc@acme.example", &[ME], &["jo@acme.example"], &[]);
        assert_eq!(
            emails(&reply_all(&m, &me(), &[]).1),
            vec!["jo@acme.example"]
        );
    }

    #[rstest]
    fn reply_all_drops_me_from_cc() {
        let m = msg(
            "marc@acme.example",
            &["lea@acme.example"],
            &["ME@Example.com"],
            &[],
        );
        assert!(reply_all(&m, &me(), &[]).1.is_empty());
    }

    #[rstest]
    fn reply_all_does_not_repeat_the_sender_in_cc() {
        let m = msg("marc@acme.example", &[ME], &["Marc@acme.example"], &[]);
        assert!(reply_all(&m, &me(), &[]).1.is_empty());
    }

    #[rstest]
    fn reply_all_does_not_repeat_an_address_listed_twice() {
        let m = msg("marc@acme.example", &[ME, "marc@acme.example"], &[], &[]);
        assert_eq!(reply_all(&m, &me(), &[]).0.len(), 1);
    }

    #[rstest]
    fn reply_to_replaces_the_sender(invoice: Message) {
        assert!(!emails(&reply_all(&invoice, &me(), &[]).0).contains(&"no-reply@invoices.example"));
    }

    #[rstest]
    fn following_up_my_own_message_keeps_its_recipients() {
        let mine = msg(ME, &["marc@acme.example"], &["jo@acme.example"], &[]);
        assert_eq!(
            emails(&reply_all(&mine, &me(), &[]).0),
            vec!["marc@acme.example"]
        );
    }

    #[rstest]
    fn following_up_my_reply_to_a_no_reply_goes_to_its_reply_to(invoice: Message) {
        let mine = msg(ME, &["no-reply@invoices.example"], &[], &[]);
        let thread = vec![invoice, mine.clone()];
        assert_eq!(
            emails(&reply_all(&mine, &me(), &thread).0),
            vec!["franck@plomberie.example"]
        );
    }

    #[rstest]
    #[case::hyphen("no-reply@tolteck.example", true)]
    #[case::plain("noreply@shop.example", true)]
    #[case::underscore("no_reply@bank.example", true)]
    #[case::dotted("no.reply@bank.example", true)]
    #[case::do_not_reply("do-not-reply@service.example", true)]
    #[case::donotreply("DoNotReply@service.example", true)]
    #[case::french("ne-pas-repondre@impots.example", true)]
    #[case::french_accent("ne-pas-répondre@ameli.example", true)]
    #[case::french_squashed("nepasrepondre@caf.example", true)]
    #[case::notifications("notifications@github.example", true)]
    #[case::mailer_daemon("MAILER-DAEMON@mx.example", true)]
    #[case::prefixed("billing-noreply@saas.example", true)]
    #[case::person("franck@plomberie.example", false)]
    #[case::notification_person("notification.team.lead@acme.example", false)]
    #[case::reply_word("reply@acme.example", false)]
    #[case::not_an_address("no-reply", false)]
    fn no_reply_detection(#[case] email: &str, #[case] want: bool) {
        assert_eq!(is_no_reply(email), want);
    }

    #[rstest]
    fn reply_to_a_no_reply_sender_is_flagged() {
        let m = msg("no-reply@invoices.example", &[ME], &[], &[]);
        assert_eq!(
            addressing(&[m], &me()).unwrap().no_reply,
            vec!["no-reply@invoices.example"]
        );
    }

    #[rstest]
    fn reply_to_reply_to_is_not_flagged(invoice: Message) {
        assert!(addressing(&[invoice], &me()).unwrap().no_reply.is_empty());
    }

    #[rstest]
    fn suggestion_prefers_the_senders_own_address(invoice_without_reply_to: Message) {
        assert_eq!(
            addressing(&[invoice_without_reply_to], &me())
                .unwrap()
                .suggestion
                .unwrap()
                .email,
            "franck.adam@plomberie.example"
        );
    }

    #[rstest]
    fn suggestion_carries_the_name_without_the_platform(invoice_without_reply_to: Message) {
        assert_eq!(
            addressing(&[invoice_without_reply_to], &me())
                .unwrap()
                .suggestion
                .unwrap()
                .name,
            "Franck ADAM"
        );
    }

    #[rstest]
    fn suggestion_falls_back_to_the_first_address() {
        let mut m = msg("noreply@shop.example", &[ME], &[], &[]);
        m.body_text = "Questions? Write to help@shop.example.".into();
        assert_eq!(
            addressing(&[m], &me()).unwrap().suggestion.unwrap().email,
            "help@shop.example"
        );
    }

    #[rstest]
    fn no_suggestion_without_an_address_in_the_text() {
        let m = msg("noreply@shop.example", &[ME], &[], &[]);
        assert_eq!(addressing(&[m], &me()).unwrap().suggestion, None);
    }

    #[rstest]
    fn no_suggestion_when_nothing_is_flagged() {
        let mut m = msg("marc@acme.example", &[ME], &[], &[]);
        m.body_text = "cc lea@acme.example".into();
        assert_eq!(addressing(&[m], &me()).unwrap().suggestion, None);
    }

    #[rstest]
    fn emails_in_text_strip_punctuation() {
        assert_eq!(
            emails_in("Write to <a@b.example>, or (c@d.example)."),
            vec!["a@b.example", "c@d.example"]
        );
    }

    fn with_id(mut m: Message, id: &str) -> Message {
        m.id = id.into();
        m
    }

    fn bounce() -> Message {
        let mut m = msg("mailer-daemon@googlemail.com", &[ME], &[], &[]);
        m.is_bounce = true;
        m
    }

    fn auto_reply() -> Message {
        let mut m = msg("lea@acme.example", &[ME], &[], &[]);
        m.subject = "Réponse automatique : Devis".into();
        m.is_auto_reply = true;
        m
    }

    fn target_id(thread: &[Message]) -> &str {
        target(thread).unwrap().id.as_str()
    }

    #[rstest]
    fn target_skips_a_bounce(invoice: Message) {
        let thread = vec![
            with_id(invoice, "invoice"),
            with_id(msg(ME, &["no-reply@invoices.example"], &[], &[]), "mine"),
            with_id(bounce(), "bounce"),
        ];
        assert_eq!(target_id(&thread), "invoice");
    }

    #[rstest]
    fn target_skips_an_auto_reply() {
        let thread = vec![
            with_id(msg("lea@acme.example", &[ME], &[], &[]), "lea"),
            with_id(auto_reply(), "auto"),
        ];
        assert_eq!(target_id(&thread), "lea");
    }

    #[rstest]
    fn target_skips_a_no_reply_sender_without_reply_to() {
        let thread = vec![
            with_id(msg("lea@acme.example", &[ME], &[], &[]), "lea"),
            with_id(msg("noreply@shop.example", &[ME], &[], &[]), "shop"),
        ];
        assert_eq!(target_id(&thread), "lea");
    }

    #[rstest]
    fn target_takes_a_no_reply_sender_with_a_person_in_reply_to(invoice: Message) {
        let thread = vec![
            with_id(msg("lea@acme.example", &[ME], &[], &[]), "lea"),
            with_id(invoice, "invoice"),
        ];
        assert_eq!(target_id(&thread), "invoice");
    }

    #[rstest]
    fn target_prefers_a_correspondent_over_my_later_message() {
        let thread = vec![
            with_id(msg("lea@acme.example", &[ME], &[], &[]), "lea"),
            with_id(msg(ME, &["lea@acme.example"], &[], &[]), "mine"),
        ];
        assert_eq!(target_id(&thread), "lea");
    }

    #[rstest]
    fn target_is_my_last_message_when_nobody_else_wrote() {
        let thread = vec![
            with_id(msg(ME, &["lea@acme.example"], &[], &[]), "first"),
            with_id(msg(ME, &["lea@acme.example"], &[], &[]), "second"),
            with_id(bounce(), "bounce"),
        ];
        assert_eq!(target_id(&thread), "second");
    }

    #[rstest]
    fn self_only_reply_keeps_my_recipients() {
        let thread = vec![
            with_id(
                msg(ME, &["lea@acme.example"], &["jo@acme.example"], &[]),
                "mine",
            ),
            with_id(bounce(), "bounce"),
        ];
        assert_eq!(
            emails(&addressing(&thread, &me()).unwrap().to),
            vec!["lea@acme.example"]
        );
    }

    #[rstest]
    fn target_falls_back_to_a_no_reply_sender_when_alone() {
        let thread = vec![with_id(
            msg("noreply@shop.example", &[ME], &[], &[]),
            "shop",
        )];
        assert_eq!(target_id(&thread), "shop");
    }

    #[rstest]
    fn target_of_only_a_bounce_is_the_bounce() {
        let thread = vec![with_id(bounce(), "bounce")];
        assert_eq!(target_id(&thread), "bounce");
    }

    #[rstest]
    fn reply_after_a_bounce_goes_to_the_reply_to(invoice: Message) {
        let thread = vec![
            with_id(invoice, "invoice"),
            with_id(msg(ME, &["no-reply@invoices.example"], &[], &[]), "mine"),
            with_id(bounce(), "bounce"),
        ];
        assert_eq!(
            emails(&addressing(&thread, &me()).unwrap().to),
            vec!["franck@plomberie.example"]
        );
    }

    #[rstest]
    fn addressing_names_the_answered_message(invoice: Message) {
        let thread = vec![with_id(invoice, "invoice"), with_id(bounce(), "bounce")];
        assert_eq!(addressing(&thread, &me()).unwrap().message_id, "invoice");
    }

    #[rstest]
    fn empty_conversation_has_no_addressing() {
        assert_eq!(addressing(&[], &me()), None);
    }
}
