//! Email → chat text: the one pipeline that turns a message into what its bubble
//! shows, only what the sender newly wrote (greeting, body, sign-off name).
//!
//! Stages, in order:
//! 1. normalise: CRLF → LF, Unicode spaces → space, invisible characters and HTML
//!    entities resolved;
//! 2. choose the source: the plain part, or the HTML part read as text when the plain
//!    part is missing, HTML source or a degraded dump of links;
//! 3. split off the reply header and quoted history ([`crate::quote::split`]);
//! 4. unwrap a forwarded email: its header block goes, its text stays, flagged;
//! 5. strip the signature (delimiter, mobile footer, contact block, disclaimer);
//! 6. strip bulk-mail boilerplate ([`crate::newsletter::clean`]) when the mail is bulk;
//! 7. strip Markdown, HTML tags, link annotations, tracking links, image placeholders;
//! 8. collapse blank runs;
//! 9. never empty when the mail has text: quoted words, then the HTML, then the whole
//!    text are tried in turn.
//!
//! The result is cached per message (`message_reading`); [`VERSION`] says which
//! pipeline wrote a cached row, so a newer one recomputes older rows.

use crate::{newsletter, quote};
use serde::{Deserialize, Serialize};

/// Version of this pipeline; bump it when its output changes, so cached readings are
/// recomputed in the background.
pub const VERSION: i64 = 3;

/// What the pipeline reads from a message.
#[derive(Debug, Clone, Copy, Default)]
pub struct Mail<'a> {
    pub subject: &'a str,
    /// The text/plain part as received (CRLF and all).
    pub text: &'a str,
    pub html: Option<&'a str>,
    /// Sent through a mailing list (a `List-Id` header).
    pub listed: bool,
}

/// What a bubble shows of a message, and what was folded away to get there.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reading {
    /// The new text: LF line ends, no blank first or last line, never empty when the
    /// message has any text.
    pub text: String,
    /// Quoted history of earlier messages was removed.
    pub has_quoted: bool,
    /// A signature, mobile footer or disclaimer was removed.
    pub has_signature: bool,
    /// The message forwards an email; `text` holds the note, then the forwarded text.
    pub forwarded: bool,
    /// Who wrote the forwarded email, as its header names them.
    pub forwarded_from: Option<String>,
    /// Bulk mail (newsletter, notification): its boilerplate was removed.
    pub bulk: bool,
    /// The message adds no words of its own: `text` is what it quotes.
    pub quoted_only: bool,
}

impl Reading {
    /// The flags packed into one integer, for storage.
    pub fn flags(&self) -> i64 {
        [
            self.has_quoted,
            self.has_signature,
            self.forwarded,
            self.bulk,
            self.quoted_only,
        ]
        .iter()
        .enumerate()
        .map(|(i, &b)| (b as i64) << i)
        .sum()
    }

    /// A stored reading, from its text, packed flags and forward sender.
    pub fn from_parts(text: String, flags: i64, forwarded_from: Option<String>) -> Self {
        let bit = |i: u32| flags & (1 << i) != 0;
        Reading {
            text,
            has_quoted: bit(0),
            has_signature: bit(1),
            forwarded: bit(2),
            forwarded_from,
            bulk: bit(3),
            quoted_only: bit(4),
        }
    }
}

/// Characters that take no room and carry no meaning in a chat bubble.
fn is_invisible(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{2064}' | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{034F}' | '\u{00AD}' | '\u{180E}'
        | '\u{FFFC}' | '\u{061C}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}')
}

/// Named HTML entities that plain parts carry ("l&#x27;homme", "d&eacute;but").
const ENTITIES: &[(&str, &str)] = &[
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("quot", "\""),
    ("apos", "'"),
    ("nbsp", " "),
    ("eacute", "é"),
    ("egrave", "è"),
    ("ecirc", "ê"),
    ("agrave", "à"),
    ("acirc", "â"),
    ("ccedil", "ç"),
    ("ocirc", "ô"),
    ("ucirc", "û"),
    ("ugrave", "ù"),
    ("icirc", "î"),
    ("iuml", "ï"),
    ("euml", "ë"),
    ("Eacute", "É"),
    ("rsquo", "’"),
    ("lsquo", "‘"),
    ("ldquo", "“"),
    ("rdquo", "”"),
    ("laquo", "«"),
    ("raquo", "»"),
    ("hellip", "…"),
    ("ndash", "–"),
    ("mdash", "—"),
    ("euro", "€"),
    ("copy", "©"),
    ("reg", "®"),
    ("bull", "•"),
    ("middot", "·"),
    ("zwnj", ""),
    ("zwj", ""),
];

/// `&…;` entities resolved; unknown ones are left as typed.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let end = after
            .char_indices()
            .take(10)
            .find(|(_, c)| *c == ';')
            .map(|(i, _)| i);
        let decoded = end.and_then(|e| {
            let name = &after[..e];
            if let Some(num) = name.strip_prefix('#') {
                let code = match num.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => num.parse().ok(),
                }?;
                let c = char::from_u32(code)?;
                Some(if c == '\u{A0}' {
                    ' '.to_string()
                } else {
                    c.to_string()
                })
            } else {
                ENTITIES
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| v.to_string())
            }
            .map(|v| (v, e))
        });
        match decoded {
            Some((v, e)) => {
                out.push_str(&v);
                rest = &after[e + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether a text is still quoted-printable (a part whose encoding was not undone):
/// lines broken by a trailing `=` and `=C3=A9`-style escapes, several of each.
fn looks_quoted_printable(s: &str) -> bool {
    if !s.contains("=\n") {
        return false;
    }
    let soft = s.matches("=\n").count();
    let b = s.as_bytes();
    let escapes = (0..b.len().saturating_sub(2))
        .filter(|&i| {
            b[i] == b'='
                && matches!(b[i + 1], b'0'..=b'9' | b'A'..=b'F')
                && matches!(b[i + 2], b'0'..=b'9' | b'A'..=b'F')
        })
        .count();
    soft >= 3 && escapes >= 3
}

/// Quoted-printable undone: soft line breaks joined, `=XX` escapes read as UTF-8 bytes.
fn decode_quoted_printable(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let hex = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    while i < b.len() {
        if b[i] == b'=' {
            if b.get(i + 1) == Some(&b'\n') {
                i += 2;
                continue;
            }
            if let (Some(h), Some(l)) = (
                b.get(i + 1).and_then(|&c| hex(c)),
                b.get(i + 2).and_then(|&c| hex(c)),
            ) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Stage 1: LF line ends, plain spaces, no invisible characters, entities resolved, no
/// trailing spaces nor leading tabs on a line.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\u{2028}' | '\u{2029}' | '\u{0085}' => out.push('\n'),
            '\u{00A0}'
            | '\u{2000}'..='\u{200A}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{3164}' => out.push(' '),
            c if is_invisible(c) => {}
            c => out.push(c),
        }
    }
    let out = if looks_quoted_printable(&out) {
        decode_quoted_printable(&out)
    } else {
        out
    };
    let out = if out.contains('&') {
        decode_entities(&out)
    } else {
        out
    };
    let mut lines = String::with_capacity(out.len());
    for (i, l) in out.split('\n').enumerate() {
        if i > 0 {
            lines.push('\n');
        }
        lines.push_str(l.trim_start_matches('\t').trim_end());
    }
    lines
}

fn has_visible(s: &str) -> bool {
    s.chars().any(|c| !c.is_whitespace() && !is_invisible(c))
}

/// An HTML body as normalised text, cut at its quoted history: (text, cut).
fn html_reading(html: &str) -> (String, bool) {
    let (cut, quoted) = quote::split_html(html);
    let text = without_navigation(&normalize(&newsletter::html_text(&cut)));
    (joined_fragments(&text), quoted)
}

/// Short labels a layout broke over lines ("15 jours pour\nchanger d'avis",
/// "Votre colis\n?") read as one line. Only a lowercase continuation (or punctuation
/// alone) joins, under a short line that ends no sentence, clause or greeting:
/// addresses ("6 rue Drouot\n75009 Paris"), sign-offs and names, bullets and table rows
/// keep their breaks.
fn joined_fragments(text: &str) -> String {
    let mut out: Vec<String> = vec![];
    for line in text.lines() {
        let t = line.trim();
        let joins = out.last().is_some_and(|prev: &String| {
            let p = prev.trim();
            let continues = t.chars().next().is_some_and(char::is_lowercase)
                || !t.is_empty() && t.chars().all(|c| "?!.,:;".contains(c));
            !p.is_empty()
                && continues
                && !p.ends_with(['.', '!', '?', ':', ';', ','])
                && !p.starts_with(['•', '-', '*'])
                && p.split_whitespace().count() <= 6
                && t.split_whitespace().count() <= 6
                && !t.starts_with("http")
        });
        match out.last_mut() {
            Some(prev) if joins => {
                // "colis" + "?" keeps the French space; a lone "," or "." hugs the word.
                if !matches!(t, "," | ".") {
                    prev.push(' ');
                }
                prev.push_str(t);
            }
            _ => out.push(line.to_string()),
        }
    }
    out.join("\n")
}

/// A web page's menu at the top of its text ("• accueil", "• tarifs", "• mon compte"…)
/// and empty bullets: a site's navigation, never what the mail says.
fn without_navigation(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let is_item = |l: &str| {
        let t = l.trim();
        t.starts_with('•') && t.trim_start_matches('•').split_whitespace().count() <= 3
    };
    let first = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
    // The menu's one-word entries ("Logout") belong to it too.
    let run = lines[first..]
        .iter()
        .take_while(|l| l.trim().is_empty() || is_item(l) || l.split_whitespace().count() == 1)
        .count();
    let menu = lines[first..first + run]
        .iter()
        .filter(|l| is_item(l))
        .count()
        >= 5;
    let start = if menu { first + run } else { 0 };
    lines[start..]
        .iter()
        .filter(|l| l.trim() != "•")
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

/// `<https://…>` and `<mailto:…>` after a link's words (how clients write a link in
/// plain text) and `<image001.png>` placeholders removed; a lone `<https://…>` keeps
/// its address.
fn without_link_annotations(text: &str) -> String {
    let mut out: Vec<String> = vec![];
    for line in text.split('\n') {
        let mut rest = line;
        let mut removed = false;
        let mut kept = String::new();
        while let Some(open) = rest.find('<') {
            let after = &rest[open + 1..];
            let Some(close) = after.find('>') else {
                break;
            };
            let inner = &after[..close];
            let low = inner.to_ascii_lowercase();
            let link = !inner.contains(' ')
                && (low.starts_with("http://")
                    || low.starts_with("https://")
                    || low.starts_with("mailto:"));
            let picture = !inner.contains(' ')
                && [".png", ".jpg", ".jpeg", ".gif", ".heic", ".webp"]
                    .iter()
                    .any(|e| low.ends_with(e));
            kept.push_str(&rest[..open]);
            if link || picture {
                removed = true;
                let words_before = !kept.trim().is_empty();
                if link && !words_before && !low.starts_with("mailto:") {
                    kept.push_str(inner);
                }
                let trimmed = kept.trim_end().len();
                kept.truncate(trimmed);
            } else {
                kept.push('<');
                kept.push_str(inner);
                kept.push('>');
            }
            rest = &after[close + 1..];
        }
        kept.push_str(rest);
        // A line that only held a picture name goes with it.
        if removed && kept.trim().is_empty() {
            continue;
        }
        out.push(kept.trim_end().to_string());
    }
    out.join("\n")
}

/// `[cid:…]`, `[image: …]` and `[https://…]` placeholders removed from a line.
fn without_bracket_placeholders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let close = after.find(']').filter(|&c| !after[..c].contains('\n'));
        let Some(close) = close else {
            out.push_str(&rest[..=open]);
            rest = after;
            continue;
        };
        let inner = after[..close].to_ascii_lowercase();
        out.push_str(&rest[..open]);
        let placeholder = inner.starts_with("cid:")
            || inner.starts_with("image:")
            || inner.starts_with("image ")
            || (inner.starts_with("http") && !inner.contains(' '));
        rest = &after[close + 1..];
        if !placeholder {
            out.push('[');
            out.push_str(&after[..close]);
            out.push(']');
        } else if out.ends_with(' ') && rest.starts_with(' ') {
            // "Plan joint [cid:…] ici": one space stays between the words.
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Warnings mail gateways put above a message; they say nothing about it.
const BANNERS: &[&str] = &[
    "vous ne recevez pas souvent de courriers",
    "you don't often get email from",
    "you don’t often get email from",
    "ce message provient d'un expéditeur externe",
    "ce message provient d’un expéditeur externe",
    "caution: this email originated from outside",
    "caution: external email",
    "[external]",
    "external email:",
];

fn without_banners(text: &str) -> String {
    let low = text.to_lowercase();
    if !BANNERS.iter().any(|b| low.contains(b)) {
        return text.to_string();
    }
    text.lines()
        .filter(|l| {
            let t = l.trim().to_lowercase();
            !BANNERS.iter().any(|b| t.starts_with(b))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// "*bold*" and "*/italic/*" marks plain-text clients put around words.
fn without_emphasis(text: &str) -> String {
    static EMPHASIS: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        // The marked words may wrap onto the next line, never past a blank one.
        regex::Regex::new(r"(^|[\s(«“'])\*/?([^\s*/](?:[^*\n]|\n[^\n*]){0,200}?[^\s*/]|[^\s*/])/?\*($|[\s.,;:!?)»”'])")
            .expect("valid regex")
    });
    if !text.contains('*') {
        return text.to_string();
    }
    EMPHASIS.replace_all(text, "$1$2$3").into_owned()
}

/// Mailing merge tags the sender never filled ("Hello *|FNAME|*,"): the unambiguous
/// template syntaxes (`*|TAG|*`, `{{ tag }}`, `%%Tag%%`) go wherever they are; `[FNAME]`
/// and `{FIRSTNAME}` only for well-known field names, so ordinary brackets stay.
fn without_merge_tags(text: &str) -> String {
    static TAG: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(concat!(
            r"[ \t]*(?:\*\|[A-Za-z0-9_: ]{1,40}\|\*",
            r"|\{\{\s*[A-Za-z_][A-Za-z0-9_.]{0,40}\s*\}\}",
            r"|%%[A-Za-z_][A-Za-z0-9_]{0,40}%%",
            r"|(?i:\[(?:fname|lname|first_?name|last_?name|firstname|prenom|prénom|nom|name)\])",
            r"|(?i:\{(?:fname|lname|first_?name|last_?name|firstname|prenom|prénom|nom|name)\}))"
        ))
        .expect("valid regex")
    });
    if !text.contains("*|")
        && !text.contains("{{")
        && !text.contains("%%")
        && !text.contains('[')
        && !text.contains('{')
    {
        return text.to_string();
    }
    let t = TAG.replace_all(text, "");
    if t.len() == text.len() {
        return text.to_string();
    }
    // "Hello ," once its tag is gone reads "Hello,".
    t.replace(" ,", ",").replace(" .", ".")
}

/// Stage 7: Markdown, tags, link annotations, tracking links, gateway banners, emphasis
/// marks, merge tags and picture placeholders removed; the words stay.
fn artefacts(text: &str) -> String {
    // Merge tags first: "*|FNAME|*" would otherwise read as emphasis marks.
    let text = &without_emphasis(&without_merge_tags(&without_banners(text)));
    let mut t = if text.contains('<') {
        without_link_annotations(&newsletter::without_tags(text))
    } else {
        text.to_string()
    };
    if t.contains('[') {
        t = without_bracket_placeholders(&t);
    }
    if t.contains("](")
        || t.contains("**")
        || t.contains("__")
        || t.contains("---")
        || t.contains('#')
    {
        t = newsletter::plain_markdown(&t);
    }
    if t.contains("http") || t.contains("www.") {
        t = newsletter::without_tracking(&t);
    }
    t
}

/// Stages 6 to 8 on a piece of text.
fn finish(text: &str, bulk: bool) -> String {
    let t = artefacts(text);
    let t = if bulk {
        newsletter::clean(&newsletter::without_link_targets(&t))
    } else {
        t
    };
    meaningful(newsletter::tidy(&t))
}

/// The text, or nothing when it is only empty quote bars and rules (">", "--").
fn meaningful(text: String) -> String {
    let says_something = text
        .chars()
        .any(|c| !c.is_whitespace() && !matches!(c, '>' | '-' | '_' | '*' | '=' | '|'));
    if !says_something {
        return String::new();
    }
    if !text.lines().any(|l| l.trim() == ">") {
        return text;
    }
    // An empty quote line in what someone wrote is a stray bar, not a quote.
    let kept: Vec<&str> = text.lines().filter(|l| l.trim() != ">").collect();
    newsletter::tidy(&kept.join("\n"))
}

/// How deep forwards inside forwards are unwrapped.
const MAX_DEPTH: usize = 4;

/// What a piece of text says of its own: stages 3 to 8.
#[derive(Default)]
struct Words {
    text: String,
    /// The quoted history under it (one `>` level removed).
    quoted: String,
    /// The text before its signature was removed (for a message that is only one).
    unsigned: String,
    signed: bool,
    forward: Option<Option<String>>,
}

fn own_words(text: &str, forward_subject: bool, bulk: bool, depth: usize) -> Words {
    let split = quote::split(text, forward_subject);
    let (fresh, signed) = quote::strip_signature(&split.fresh);
    let mut w = Words {
        text: finish(&fresh, bulk),
        quoted: split.quoted,
        unsigned: split.fresh,
        signed,
        forward: None,
    };
    if let Some(f) = split.forward {
        w.forward = Some(f.from);
        let inner = if depth < MAX_DEPTH {
            // Inside a forward, a separator or header block opens the next forward.
            let inner = own_words(&f.body, true, bulk, depth + 1);
            if inner.text.is_empty() {
                read_quoted(&inner.quoted, bulk, depth + 1)
            } else {
                inner.text
            }
        } else {
            finish(&f.body, bulk)
        };
        w.text = match (w.text.is_empty(), inner.is_empty()) {
            (_, true) => w.text,
            (true, false) => inner,
            (false, false) => format!("{}\n\n{inner}", w.text),
        };
    }
    w
}

/// What a quoted history says, read like a message of its own.
fn read_quoted(quoted: &str, bulk: bool, depth: usize) -> String {
    if quoted.is_empty() || depth > MAX_DEPTH {
        return String::new();
    }
    let w = own_words(quoted, false, bulk, depth + 1);
    if !w.text.is_empty() {
        return w.text;
    }
    let deeper = read_quoted(&w.quoted, bulk, depth + 1);
    if deeper.is_empty() {
        finish(quoted, bulk)
    } else {
        deeper
    }
}

/// Reads a message the way its bubble shows it.
pub fn read(m: &Mail) -> Reading {
    let plain = normalize(m.text);
    let html = m.html.filter(|h| !h.trim().is_empty());
    let forward_subject = quote::is_forward(m.subject);
    // Stage 2: the source. A degraded plain part that still holds a reply or a forward
    // keeps its structure; the HTML would lose it.
    let source_html = newsletter::looks_like_html(&plain);
    let plain_split = quote::split(&plain, forward_subject);
    // Anything split off (a quote, an attribution over an inline reply, a forward).
    let structured = !plain_split.quoted.is_empty()
        || plain_split.forward.is_some()
        || plain_split.fresh != plain.trim();
    let mut from_html = None;
    let use_html = match html {
        _ if source_html => true,
        Some(_) if !has_visible(&plain) => true,
        Some(_) if !structured && newsletter::degraded(&plain) => true,
        // A plain part that opens on the HTML's style sheet is a dump of it.
        Some(_) if newsletter::opens_on_css(&plain) => true,
        // The HTML marks a quote the plain part lost (a text flattened from the HTML
        // runs the quote into the reply): the HTML says where the reply ends.
        // Only when the plain part names a quoted email: a blockquote alone may be an
        // inline quote with answers under it.
        Some(h)
            if !structured && quote::mentions_attribution(&plain) && quote::has_html_quote(h) =>
        {
            let (text, cut) = html_reading(h);
            let use_it = cut && has_visible(&text);
            from_html = use_it.then_some((text, cut));
            use_it
        }
        _ => false,
    };
    let (body, html_quoted) = match from_html {
        Some(r) => r,
        None if use_html => html_reading(html.unwrap_or(m.text)),
        None => (plain.clone(), false),
    };
    // A style sheet is never text, whichever part it came in.
    let body = newsletter::without_css(&body);
    let bulk = m.listed || newsletter::is_bulk(&body, html);
    // In a newsletter, ">" marks buttons and pull quotes, never quoted history; a list
    // post (`listed`) is a person's reply and keeps its quotes.
    let body = if bulk && !m.listed && body.contains('>') {
        body.lines()
            .map(|l| l.trim_start().trim_start_matches('>').trim_start())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        body
    };
    let w = own_words(&body, forward_subject, bulk, 0);
    let mut r = Reading {
        has_quoted: html_quoted || !w.quoted.is_empty(),
        has_signature: w.signed,
        forwarded: w.forward.is_some(),
        forwarded_from: w.forward.clone().flatten(),
        bulk,
        ..Default::default()
    };
    if !w.text.is_empty() {
        r.text = w.text;
        return r;
    }
    // Stage 9: never a blank bubble. What it quotes, then the signature it is made of,
    // then its HTML, then all of its text.
    r.text = read_quoted(&w.quoted, bulk, 0);
    r.quoted_only = !r.text.is_empty();
    if r.text.is_empty() && w.signed {
        // Only a signature: its first line (the name, or the word it opens on), without
        // the "-- " delimiter.
        let first = w
            .unsigned
            .lines()
            .find(|l| !matches!(l.trim(), "" | "--" | "—"))
            .unwrap_or("");
        r.text = finish(first, bulk);
    }
    if r.text.is_empty()
        && let Some(h) = html.filter(|_| !use_html)
    {
        r.text = finish(&html_reading(h).0, bulk);
    }
    if r.text.is_empty() {
        r.text = meaningful(newsletter::tidy(&artefacts(&body)));
    }
    if r.text.is_empty() && !use_html {
        r.text = meaningful(newsletter::tidy(&plain));
    }
    if r.text.is_empty()
        && let Some(h) = html
    {
        r.text = meaningful(newsletter::tidy(&normalize(&newsletter::html_text(h))));
    }
    r
}

#[cfg(test)]
mod tests;
