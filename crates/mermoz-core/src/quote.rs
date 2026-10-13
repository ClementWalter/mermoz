//! Separates what a message says from the history it quotes, so threads read
//! as a conversation instead of a pile of nested replies. Forwards are left
//! whole: there the "quoted" part is the content.

/// Attribution openers ("On …", "Le …") and closers ("wrote:", "a écrit :") per language.
const ATTR_START: &[&str] = &["on ", "le ", "am ", "el ", "il ", "op ", "em ", "den "];
const ATTR_END: &[&str] = &[
    "wrote:",
    "a écrit :",
    "a écrit:",
    "a ecrit :",
    "schrieb:",
    "escribió:",
    "ha scritto:",
    "schreef:",
    "escreveu:",
    "skrev:",
];
const HEADER_FROM: &[&str] = &["from:", "de :", "de:", "von:"];
const HEADER_REST: &[&str] = &[
    "sent:",
    "date:",
    "envoyé :",
    "envoyé:",
    "to:",
    "à :",
    "à:",
    "subject:",
    "objet :",
    "objet:",
    "gesendet:",
    "an:",
    "betreff:",
];
const FORWARD: &[&str] = &[
    "forwarded message",
    "begin forwarded message",
    "message transféré",
    "weitergeleitete nachricht",
];
const ORIGINAL: &[&str] = &[
    "-----original message-----",
    "----- original message -----",
    "-------- message d'origine --------",
    "-----message d'origine-----",
];

fn is_attribution(line: &str, next: &str) -> bool {
    let l = line.trim().to_lowercase();
    if !ATTR_START.iter().any(|s| l.starts_with(s)) {
        return false;
    }
    let joined = format!("{l} {}", next.trim().to_lowercase());
    ATTR_END
        .iter()
        .any(|e| l.ends_with(e) || joined.ends_with(e))
}

/// Outlook-style header block: a From line followed closely by Sent/To/Subject lines.
fn is_header_block(lines: &[&str], i: usize) -> bool {
    let l = lines[i].trim().to_lowercase();
    if !HEADER_FROM.iter().any(|h| l.starts_with(h)) {
        return false;
    }
    let following = lines.iter().skip(i + 1).take(5).filter(|x| {
        let x = x.trim().to_lowercase();
        HEADER_REST.iter().any(|h| x.starts_with(h))
    });
    following.count() >= 2
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 10 && t.chars().all(|c| c == '_' || c == '-')
}

/// Mobile footers and boilerplate that add nothing to a conversation.
const FOOTERS: &[&str] = &[
    "sent from my iphone",
    "sent from my ipad",
    "envoyé de mon iphone",
    "envoyé de mon ipad",
    "get outlook for ios",
    "get outlook for android",
    "sent from outlook",
    "sent from my android",
    "sent with proton mail",
    "télécharger outlook",
];
const DISCLAIMERS: &[&str] = &[
    "this email and any attachments",
    "this e-mail and any attachments",
    "this message is confidential",
    "this message and any attachments",
    "ce message et toutes les pièces jointes",
    "ce courriel et toutes les pièces jointes",
    "le présent message",
];

/// Removes the "-- " signature block, mobile footers and trailing legal disclaimers.
pub fn strip_signature(text: &str) -> (String, bool) {
    let lines: Vec<&str> = text.lines().collect();
    let mut cut = lines.len();
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_end();
        let low = t.trim().to_lowercase();
        // RFC 3676 signature delimiter, or a footer/disclaimer starting a line.
        if t == "--"
            || t == "-- "
            || FOOTERS.iter().any(|f| low.starts_with(f))
            || DISCLAIMERS.iter().any(|d| low.starts_with(d))
        {
            cut = i;
            break;
        }
    }
    if cut == lines.len() {
        cut = contact_block(&lines).unwrap_or(cut);
    }
    if cut == lines.len() || lines[..cut].iter().all(|l| l.trim().is_empty()) {
        return (text.trim_end().to_string(), false);
    }
    (lines[..cut].join("\n").trim_end().to_string(), true)
}

/// A line of contact details: an address, a phone number or a web page on its own.
fn is_contact_line(line: &str) -> bool {
    let t = line.trim();
    let low = t.to_lowercase();
    let words = t.split_whitespace().count();
    let email = t.contains('@') && t.contains('.') && words <= 4;
    let web = (low.starts_with("http") || low.starts_with("www.") || low.contains("linkedin.com/"))
        && words <= 2;
    // "Tél : +33 6 12 34 56 78", "M. 06.12.34.56.78": a label, then digits.
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
    let rest = t.trim_start_matches(|c: char| {
        c.is_alphabetic() || c == '.' || c == ':' || c == ' ' || c == '(' || c == ')'
    });
    let phone = digits >= 8
        && t.len() <= 40
        && rest
            .chars()
            .all(|c| c.is_ascii_digit() || " +.-()/".contains(c));
    email || web || phone
}

/// A short line a signature carries: a name, a title, a company.
fn is_name_line(line: &str) -> bool {
    let t = line.trim();
    let words = t.split_whitespace().count();
    (1..=6).contains(&words)
        && t.chars().count() <= 50
        && !t.ends_with([',', '.', '!', '?', ':', ';'])
}

/// Where a trailing block of contact details starts ("Name / email / +33 phone"): at
/// least two lines, one of them an address, phone or web page, after some content.
fn contact_block(lines: &[&str]) -> Option<usize> {
    let mut start = lines.len();
    let (mut kept, mut strong) = (0, 0);
    for i in (0..lines.len()).rev() {
        let l = lines[i];
        if l.trim().is_empty() {
            if kept > 0 && i > 0 && lines[i - 1].trim().is_empty() {
                break;
            }
            continue;
        }
        let contact = is_contact_line(l);
        if !contact && !is_name_line(l) {
            break;
        }
        strong += contact as usize;
        kept += 1;
        start = i;
        if kept >= 6 {
            break;
        }
    }
    let content_before = lines[..start].iter().any(|l| !l.trim().is_empty());
    (kept >= 2 && strong >= 1 && content_before).then_some(start)
}

/// What a reader needs: new content without quoted history, signature or boilerplate.
/// Returns (content, whether anything was hidden).
pub fn readable(text: &str) -> (String, bool) {
    let (fresh, quoted) = split_text(text);
    let (clean, signed) = strip_signature(&fresh);
    (clean, quoted || signed)
}

/// Splits plain text into (new content, whether quoted history was removed).
pub fn split_text(text: &str) -> (String, bool) {
    let lines: Vec<&str> = text.lines().collect();
    let lower = text.to_lowercase();
    let mut cut = None;
    for i in 0..lines.len() {
        let next = lines.get(i + 1).copied().unwrap_or("");
        let t = lines[i].trim().to_lowercase();
        let quoted_tail = t.starts_with('>')
            && lines[i..]
                .iter()
                .all(|l| l.trim().is_empty() || l.trim_start().starts_with('>'));
        if is_attribution(lines[i], next)
            || is_header_block(&lines, i)
            || ORIGINAL.iter().any(|o| t.starts_with(o))
            || quoted_tail
        {
            cut = Some(i);
            break;
        }
    }
    let Some(mut cut) = cut else {
        return (text.trim_end().to_string(), false);
    };
    let fresh_lower: String = lines[..cut].join("\n").to_lowercase();
    if FORWARD.iter().any(|f| fresh_lower.contains(f))
        || (cut == 0 && FORWARD.iter().any(|f| lower.contains(f)))
    {
        return (text.trim_end().to_string(), false);
    }
    // Drop the separator rules and blank lines that sat just above the quote.
    while cut > 0 && (lines[cut - 1].trim().is_empty() || is_rule(lines[cut - 1])) {
        cut -= 1;
    }
    if cut == 0 {
        return (text.trim_end().to_string(), false);
    }
    (lines[..cut].join("\n").trim_end().to_string(), true)
}

/// Markers clients put around quoted history in HTML replies.
const HTML_MARKERS: &[&str] = &[
    "class=\"gmail_quote",
    "<blockquote type=\"cite\"",
    "id=\"appendonsend\"",
    "id=\"divrplyfwdmsg\"",
    "id=\"x_divrplyfwdmsg\"",
    "id=\"mail-editor-reference-message-container\"",
    "class=\"moz-cite-prefix\"",
    "class=\"yahoo_quoted\"",
    "<hr id=\"stopspelling\"",
];

/// The list preview of a message: its new text on one line, without the CSS and the
/// invisible "preheader" padding some newsletters put first.
pub fn snippet(body: &str) -> String {
    const INVISIBLE: &[char] = &[
        '\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}', '\u{034F}', '\u{00AD}',
        '\u{180E}',
    ];
    let (text, _) = readable(body);
    let looks_like_css = |l: &str| {
        let t = l.trim();
        t.starts_with('@')
            && (t.contains("url(")
                || t.contains('{')
                || t.starts_with("@media")
                || t.starts_with("@import"))
            || t.ends_with('{')
            || t == "}"
            || (t.contains('{') && t.contains('}') && t.contains(':'))
    };
    text.lines()
        .filter(|l| !looks_like_css(l))
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !INVISIBLE.contains(c))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
}

/// Whether the subject marks a forward ("TR:", "Fwd:", "WG:"…): the forwarded email,
/// though it looks like a quoted reply, is then the content.
pub fn is_forward(subject: &str) -> bool {
    let s = subject.trim_start().to_lowercase();
    let head = s.split(':').next().unwrap_or("").trim();
    s.contains(':') && ["tr", "fwd", "fw", "wg", "rv", "enc", "i"].contains(&head)
}

/// Cuts an HTML body at the first quote marker; the sanitizer repairs the open tags.
pub fn split_html(html: &str) -> (String, bool) {
    // ASCII-only lowercasing keeps byte offsets identical to `html` (the markers are ASCII).
    let lower = html.to_ascii_lowercase();
    let Some(pos) = HTML_MARKERS.iter().filter_map(|m| lower.find(m)).min() else {
        return (html.to_string(), false);
    };
    let start = lower[..pos].rfind('<').unwrap_or(pos);
    // Gmail wraps forwards in the same container as quotes; a forward is content.
    let mut end = (start + 800).min(lower.len());
    while !lower.is_char_boundary(end) {
        end -= 1;
    }
    let head = &lower[start..end];
    if FORWARD.iter().any(|f| head.contains(f)) || lower[..start].trim().is_empty() {
        return (html.to_string(), false);
    }
    (html[..start].to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const OUTLOOK_FR: &str = "Merci beaucoup !!\n\n________________________________\nFrom: Clément Walter <c@x.com>\nSent: Monday, 29 June 2026 11:07:21\nTo: Valérie <v@y.eu>\nSubject: Re: Projet\n\nBonjour Valérie\n";
    const GMAIL_FR: &str = "Parfait, merci.\n\nLe dim. 11 oct. 2026 à 08:12, Marc Petit <marc@x.com> a écrit :\n> Hello\n> there\n";
    const GMAIL_EN_WRAPPED: &str = "Sounds good.\n\nOn Sun, Oct 11, 2026 at 8:12 AM Marc Petit <marc@x.com>\nwrote:\n\n> Hello\n";
    const ANGLE_TAIL: &str = "Yes.\n\n> earlier text\n> more\n";
    const FORWARD_TXT: &str = "FYI\n\n---------- Forwarded message ---------\nFrom: Bank <b@x.com>\nDate: Sun, Oct 11\nSubject: Statement\nTo: me\n\nYour statement\n";
    const INLINE_REPLY: &str = "> question one?\nanswer one\n> question two?\nanswer two\n";

    #[rstest]
    #[case::outlook_header_block(OUTLOOK_FR, "Merci beaucoup !!")]
    #[case::gmail_french_attribution(GMAIL_FR, "Parfait, merci.")]
    #[case::wrapped_english_attribution(GMAIL_EN_WRAPPED, "Sounds good.")]
    #[case::trailing_angle_quotes(ANGLE_TAIL, "Yes.")]
    fn keeps_only_new_text(#[case] input: &str, #[case] want: &str) {
        assert_eq!(split_text(input).0, want);
    }

    #[rstest]
    #[case::outlook(OUTLOOK_FR, true)]
    #[case::forward_kept_whole(FORWARD_TXT, false)]
    #[case::inline_reply_kept_whole(INLINE_REPLY, false)]
    #[case::plain(&"Just a note.", false)]
    fn reports_whether_quote_was_removed(#[case] input: &str, #[case] want: bool) {
        assert_eq!(split_text(input).1, want);
    }

    #[rstest]
    #[case::rfc_delimiter("See you.\n-- \nMarc Petit\nCFO", "See you.")]
    #[case::iphone_footer("Ok!\n\nSent from my iPhone", "Ok!")]
    #[case::french_footer("D'accord\n\nEnvoyé de mon iPhone", "D'accord")]
    #[case::disclaimer("Done.\n\nThis email and any attachments are confidential.", "Done.")]
    #[case::plain_kept("Line one\n--\n", "Line one")]
    fn signatures_are_hidden(#[case] input: &str, #[case] want: &str) {
        assert_eq!(readable(input).0, want);
    }

    #[rstest]
    fn message_that_is_only_a_signature_is_kept() {
        assert_eq!(strip_signature("-- \nMarc").0, "-- \nMarc");
    }

    #[rstest]
    fn gmail_html_quote_is_cut() {
        let html = r#"<div dir="ltr">Thanks!</div><br><div class="gmail_quote"><div>On Sun Marc wrote:</div><blockquote>old</blockquote></div>"#;
        assert_eq!(split_html(html).0, r#"<div dir="ltr">Thanks!</div><br>"#);
    }

    #[rstest]
    fn outlook_html_quote_is_cut() {
        let html =
            r#"<p>Merci</p><div id="appendonsend"></div><hr><div id="divRplyFwdMsg">From: x</div>"#;
        assert_eq!(split_html(html).0, "<p>Merci</p>");
    }

    #[rstest]
    #[case::css_and_preheader(
        "@import url('https://fonts.googleapis.com/css?family=Montserrat'); \u{200C} \u{200C} \u{200C}\nDemande de place en crèche\n\nBonjour",
        "Demande de place en crèche Bonjour"
    )]
    #[case::style_block("body { color: red; }\nHello there", "Hello there")]
    #[case::plain("Just a note.", "Just a note.")]
    fn snippets_skip_css_and_invisible_padding(#[case] body: &str, #[case] want: &str) {
        assert_eq!(snippet(body), want);
    }

    #[rstest]
    #[case::outlook_french("TR: Exceptional Event", true)]
    #[case::gmail("Fwd: Statement", true)]
    #[case::outlook("FW: report", true)]
    #[case::german("WG: Termin", true)]
    #[case::spaced_colon("TR : invitation", true)]
    #[case::reply_to_a_forward("RE: TR: invitation", false)]
    #[case::plain("Trip to Rome", false)]
    #[case::no_colon("Fwd", false)]
    fn forward_subjects(#[case] subject: &str, #[case] want: bool) {
        assert_eq!(is_forward(subject), want);
    }

    #[rstest]
    fn accent_at_the_lookahead_edge_does_not_panic() {
        // The 800-byte forward lookahead ends inside "à".
        let tail = format!(
            "{}à and more",
            "x".repeat(799 - r#"<div class="gmail_quote">"#.len())
        );
        let html = format!(r#"<p>Hi</p><div class="gmail_quote">{tail}</div>"#);
        assert_eq!(split_html(&html).0, "<p>Hi</p>");
    }

    #[rstest]
    fn html_without_marker_is_untouched() {
        assert!(!split_html("<p>Hello</p>").1);
    }

    #[rstest]
    #[case::name_email_phone(
        "Here is the plan.\n\nNora Lind\nnora@agency.example\n+33 6 12 34 56 78",
        "Here is the plan."
    )]
    #[case::labelled_phone(
        "See you then.\n\nNora Lind\nAgence Lind\nTél : +33 6 12 34 56 78",
        "See you then."
    )]
    #[case::sign_off_kept(
        "Thanks for the quick answer.\n\nMerci,\nNora Lind\nnora@agency.example",
        "Thanks for the quick answer.\n\nMerci,"
    )]
    #[case::website(
        "Draft attached.\n\nSam Ito\nStudio Ito\nwww.studio.example",
        "Draft attached."
    )]
    #[case::dash_delimiter("Talk soon.\n--\nSam", "Talk soon.")]
    fn trailing_contact_blocks_are_signatures(#[case] text: &str, #[case] want: &str) {
        assert_eq!(strip_signature(text).0, want);
    }

    #[rstest]
    #[case::bare_name_sign_off("Let's talk tomorrow.\nNora")]
    #[case::one_link("Here is the booking page:\nhttps://notary.example/booking")]
    #[case::only_a_phone("+33 6 12 34 56 78")]
    #[case::number_in_a_sentence("Call me at +33 6 12 34 56 78 after five.")]
    #[case::short_list("Agenda:\nBudget\nHiring\nOffice move")]
    fn content_is_not_mistaken_for_a_signature(#[case] text: &str) {
        assert_eq!(strip_signature(text).0, text);
    }
}
