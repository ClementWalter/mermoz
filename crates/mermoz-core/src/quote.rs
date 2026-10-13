//! Quoted history, forwards and signatures: the stages of [`crate::reading`] that
//! separate what a person newly wrote from what their mail client appended. Input text
//! is already normalised (LF line ends, plain spaces, no invisible characters).

/// Attribution openers ("On …", "Le …", "Il giorno …", "Quoting …") per language.
const ATTR_START: &[&str] = &[
    "on ",
    "le ",
    "am ",
    "el ",
    "il ",
    "op ",
    "em ",
    "den ",
    "w dniu ",
    "quoting ",
    "citando ",
    "zitat von ",
];
/// Attribution closers ("wrote:", "a écrit :") per language, spaces before ':' folded.
const ATTR_END: &[&str] = &[
    "wrote:",
    "a écrit:",
    "a ecrit:",
    "écrit:",
    "schrieb:",
    "escribió:",
    "ha scritto:",
    "schreef:",
    "escreveu:",
    "skrev:",
    "napisał:",
    "napisał(a):",
    "kirjoitti:",
    "geschreven:",
];
/// Headers that open a quoted or forwarded email (Outlook, Apple Mail, Thunderbird).
const HEADER_FROM: &[&str] = &["from:", "de:", "von:", "da:", "van:"];
const HEADER_REST: &[&str] = &[
    "sent:",
    "date:",
    "envoyé:",
    "envoyé le:",
    "to:",
    "à:",
    "a:",
    "cc:",
    "subject:",
    "objet:",
    "gesendet:",
    "datum:",
    "data:",
    "date d'envoi:",
    "sujet:",
    "pour:",
    "copie à:",
    "an:",
    "betreff:",
    "inviato:",
    "oggetto:",
    "para:",
    "asunto:",
    "enviado:",
    "fecha:",
    "reply-to:",
    "répondre à:",
];
/// Lines that introduce a forwarded email.
const FORWARD: &[&str] = &[
    "forwarded message",
    "begin forwarded message",
    "message transféré",
    "début du message réexpédié",
    "message réexpédié",
    "weitergeleitete nachricht",
    "messaggio inoltrato",
    "mensaje reenviado",
    "mensagem encaminhada",
    "doorgestuurd bericht",
    "inizio messaggio inoltrato",
];
/// "-----Original Message-----" separators, without their dashes.
const ORIGINAL: &[&str] = &[
    "original message",
    "message d'origine",
    "message d’origine",
    "mail original",
    "message original",
    "ursprüngliche nachricht",
    "messaggio originale",
    "mensaje original",
    "mensagem original",
    "reply message",
    "original message follows",
    "message de réponse",
];

/// Whether `line`, leading blanks, `>` and `*` aside, starts with one of the lowercase
/// `prefixes`, ignoring case and any spaces before a ':'. Allocation-free: it runs on
/// every line of every message.
fn starts_ci(line: &str, prefixes: &[&str]) -> bool {
    let line = line.trim_start_matches([' ', '\t', '*']);
    let Some(&b) = line.as_bytes().first() else {
        return false;
    };
    let first = b.to_ascii_lowercase();
    prefixes.iter().any(|p| {
        // Most lines differ from every marker at their first (ASCII) byte.
        if b.is_ascii() && p.as_bytes()[0] != first {
            return false;
        }
        let mut it = line
            .chars()
            .flat_map(char::to_lowercase)
            .filter(|c| *c != '*')
            .peekable();
        p.chars().all(|c| {
            // "De :", "Objet :": the space French puts before a colon.
            if c == ':' {
                while it.peek() == Some(&' ') {
                    it.next();
                }
            }
            it.next() == Some(c)
        })
    })
}

/// Whether `s` ends with one of the lowercase `ends`, spaces before ':' folded.
fn ends_ci(s: &str, ends: &[&str]) -> bool {
    let t = s.trim_end().to_lowercase().replace(" :", ":");
    ends.iter().any(|e| t.ends_with(e))
}

/// Whether the line is quoted with `>`.
fn is_quoted(line: &str) -> bool {
    line.trim_start().starts_with('>')
}

/// How many lines (1 to 3) an attribution starting at `i` spans, if it starts one:
/// "On Sun, Oct 11, 2026 at 8:12 AM Marc <m@x.com>\nwrote:" wraps.
fn attribution(lines: &[&str], i: usize) -> Option<usize> {
    let l = lines[i].trim();
    if l.is_empty() || l.len() > 400 {
        return None;
    }
    let opener = starts_ci(l, ATTR_START);
    // "Marc Petit <m@x.com> a écrit :" without an opener: an address or a time says it.
    let addressed = l.contains('@') || l.contains(':') && l.chars().any(|c| c.is_ascii_digit());
    // Old Gmail: "2011/9/20 Marc Petit <marc@x.example>" right above the quote.
    // Its closing ">" may wrap onto the next line, where it reads as a quote mark.
    let dated = l.len() >= 10
        && l.as_bytes()[..4].iter().all(u8::is_ascii_digit)
        && l.as_bytes()[4] == b'/'
        && l.contains('@')
        && l.contains('<');
    if dated {
        let quote_follows = lines[i + 1..]
            .iter()
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| is_quoted(l));
        return quote_follows.then_some(1);
    }
    if !opener && !addressed {
        // "Marc Petit a écrit :" alone, right above the quote.
        let quote_follows = lines[i + 1..]
            .iter()
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| is_quoted(l));
        return (l.chars().count() <= 100 && quote_follows && ends_ci(l, ATTR_END)).then_some(1);
    }
    let mut joined = String::from(l);
    for span in 1..=3 {
        if ends_ci(&joined, ATTR_END)
            || opener && starts_ci(l, &["quoting "]) && joined.ends_with(':')
        {
            return Some(span);
        }
        if !opener {
            return None;
        }
        let next = lines.get(i + span)?.trim();
        if next.is_empty() || next.len() > 200 {
            return None;
        }
        joined.push(' ');
        joined.push_str(next);
    }
    None
}

/// Outlook-style header block: a From line followed closely by Sent/To/Subject lines.
/// Returns how many lines it spans, up to the blank line that ends it.
fn header_block(lines: &[&str], i: usize) -> Option<usize> {
    if !starts_ci(lines[i], HEADER_FROM) {
        return None;
    }
    let rest = lines
        .iter()
        .skip(i + 1)
        .take(6)
        .filter(|x| starts_ci(x, HEADER_REST))
        .count();
    if rest < 2 {
        return None;
    }
    Some(header_len(&lines[i..]))
}

/// A wrapped line of recipients: `Ana <ana@x.example>, Max <`.
fn is_address_list(line: &str) -> bool {
    let t = line.trim();
    t.contains('@')
        && (t.starts_with('<') || t.contains(">,") || t.ends_with(',') || t.ends_with('<'))
}

fn is_header(line: &str) -> bool {
    starts_ci(line, HEADER_FROM) || starts_ci(line, HEADER_REST)
}

/// The number of header lines at the start of `lines`, with their wrapped
/// continuations (a long subject or address list breaks onto the next line) and the
/// blank lines some clients put between them.
fn header_len(lines: &[&str]) -> usize {
    let mut n = 0;
    while n < lines.len() {
        if is_header(lines[n]) {
            n += 1;
            continue;
        }
        if n == 0 {
            break;
        }
        // A continuation or a blank counts only when another header follows it.
        let next = lines[n..].iter().skip(1).take(2).position(|l| is_header(l));
        let wrapped = !lines[n].trim().is_empty() && lines[n].trim().chars().count() <= 120;
        match next {
            Some(0) if wrapped || lines[n].trim().is_empty() => n += 1,
            Some(1) if lines[n].trim().is_empty() || wrapped && lines[n + 1].trim().is_empty() => {
                n += 2
            }
            _ if lines[n].starts_with([' ', '\t']) && wrapped => n += 1,
            // An address list goes on after a trailing comma or an opened "<", and its
            // wrapped lines are addresses themselves.
            _ if wrapped && lines[n - 1].trim_end().ends_with([',', '<', ';']) => n += 1,
            _ if wrapped && is_address_list(lines[n]) => n += 1,
            // The last header wrapped at the client's line width, right before the body.
            None if wrapped
                && is_header(lines[n - 1])
                && lines[n - 1].chars().count() >= 60
                && lines.get(n + 1).is_none_or(|l| l.trim().is_empty()) =>
            {
                n += 1
            }
            _ => break,
        }
    }
    n
}

fn is_original_separator(line: &str) -> bool {
    let t = line.trim().trim_start_matches('>').trim();
    if !t.starts_with("--") && !t.starts_with("__") {
        return false;
    }
    let core = t.trim_matches(['-', '_', ' ']).to_lowercase();
    ORIGINAL.contains(&core.as_str())
}

/// A line that introduces a forwarded email ("---------- Forwarded message ---------").
fn is_forward_marker(line: &str) -> bool {
    let t = line.trim().trim_start_matches(['>', ' ']);
    if t.is_empty() || t.chars().count() > 80 {
        return false;
    }
    let low = t.to_lowercase();
    FORWARD.iter().any(|f| low.contains(f))
}

fn is_rule(line: &str) -> bool {
    let t = line.trim();
    t.chars().count() >= 10
        && t.chars()
            .all(|c| c == '_' || c == '-' || c == '—' || c == '━')
}

/// Whether the subject marks a forward ("TR:", "Fwd:", "WG:"…): the forwarded email,
/// though it looks like a quoted reply, is then the content.
pub fn is_forward(subject: &str) -> bool {
    let mut s = subject.trim_start().to_lowercase();
    // "[p10] Fwd: …": a mailing list's tag comes first.
    while s.starts_with('[')
        && let Some(end) = s.find(']')
    {
        s = s[end + 1..].trim_start().to_string();
    }
    let head = s.split(':').next().unwrap_or("").trim();
    s.contains(':') && ["tr", "fwd", "fw", "wg", "rv", "enc", "i"].contains(&head)
}

/// A forwarded email inside a message.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Forward {
    /// Who wrote the forwarded email, as its From header names them.
    pub from: Option<String>,
    /// Its text, header block removed.
    pub body: String,
}

/// A message's text split into what it adds and what it carries along.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Split {
    /// What the sender wrote: the text above the quoted history (or below it, for a
    /// bottom-posted reply).
    pub fresh: String,
    /// The quoted history, one level of `>` removed (empty when there is none).
    pub quoted: String,
    /// The forwarded email, when the message forwards one.
    pub forward: Option<Forward>,
}

/// One level of `>` quoting removed from each line.
fn unquote(lines: &[&str]) -> String {
    lines
        .iter()
        .map(|l| {
            let t = l.trim_start();
            match t.strip_prefix('>') {
                Some(r) => r.strip_prefix(' ').unwrap_or(r),
                None => l,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn joined(lines: &[&str]) -> String {
    lines.join("\n").trim().to_string()
}

/// The forwarded email in `lines` (after its marker): quoting removed, header parsed.
fn parse_forward(lines: &[&str]) -> Forward {
    let content: Vec<&&str> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    let quoted = content.iter().filter(|l| is_quoted(l)).count();
    let text = if !content.is_empty() && quoted * 2 >= content.len() {
        unquote(lines)
    } else {
        lines.join("\n")
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
    let n = header_len(&lines[start..]);
    let from = lines[start..start + n]
        .iter()
        .find(|l| starts_ci(l, HEADER_FROM))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| display_name(v))
        .filter(|v| !v.is_empty());
    Forward {
        from,
        body: joined(&lines[start + n..]),
    }
}

/// "Bank <b@x.com>" → "Bank"; "<b@x.com>" → "b@x.com".
fn display_name(v: &str) -> String {
    let v = v.trim().trim_matches('*').trim();
    let name = v
        .split('<')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(['"', '\''])
        .trim();
    if name.is_empty() {
        v.trim_matches(['<', '>', ' '])
            .split('>')
            .next()
            .unwrap_or("")
            .to_string()
    } else {
        name.to_string()
    }
}

/// Lines after a quoted block that only sign it off: a few short lines, no sentence.
fn signs_off(lines: &[&str]) -> bool {
    let content: Vec<&&str> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    let delimited = content.first().is_some_and(|l| l.trim() == "--") && content.len() <= 12;
    // A sentence under the quote is an answer, not a sign-off.
    let short = |l: &&&str| {
        let t = l.trim();
        t.chars().count() <= 80 && !t.ends_with(['.', '?', '!'])
    };
    delimited || content.len() <= 4 && content.iter().all(short)
}

/// Where the quoted history starts: (first line of the cut, first line of the quote).
fn quote_cut(lines: &[&str], forward_subject: bool) -> Option<(usize, usize, bool)> {
    let last = lines.iter().rposition(|l| is_quoted(l));
    // quoted_to_end[k]: lines k..=last are all quoted, blank, or a long quoted line's
    // wrap; worked out once, from the end, so long texts stay linear.
    let mut quoted_to_end = vec![false; lines.len() + 1];
    if let Some(last) = last {
        quoted_to_end[last + 1] = true;
        for k in (0..=last).rev() {
            let l = lines[k];
            let inside = l.trim().is_empty()
                || is_quoted(l)
                || k > 0 && is_quoted(lines[k - 1]) && lines[k - 1].chars().count() >= 60;
            quoted_to_end[k] = inside && quoted_to_end[k + 1];
        }
    }
    for i in 0..lines.len() {
        if is_forward_marker(lines[i]) {
            return Some((i, i + 1, true));
        }
        if let Some(n) = attribution(lines, i) {
            return Some((i, i + n, false));
        }
        if let Some(n) = header_block(lines, i) {
            return Some((i, i + n, forward_subject));
        }
        if is_original_separator(lines[i]) {
            return Some((i, i + 1, forward_subject));
        }
        if is_quoted(lines[i]) {
            // The rest is quoted, blank, or a short sign-off under the quote.
            let last = last.unwrap_or(i);
            if !quoted_to_end[i] {
                continue;
            }
            // A quote that opens on an email's headers is that email: forwarded under a
            // forward's subject, quoted history otherwise.
            if quotes_headers(&lines[i..]) {
                return Some((i, i, forward_subject));
            }
            // Quotes higher up with answers between them: an inline reply, read whole.
            let answered_above = lines[..i].iter().any(|l| is_quoted(l));
            if !answered_above && signs_off(&lines[last + 1..]) {
                return Some((i, i, false));
            }
        }
    }
    None
}

/// Whether a `>` quote starts with a header block within its first lines.
fn quotes_headers(lines: &[&str]) -> bool {
    let unq: Vec<&str> = lines
        .iter()
        .take(12)
        .map(|l| {
            let t = l.trim_start();
            t.strip_prefix('>').map_or(t, |r| r.trim_start())
        })
        .collect();
    (0..unq.len().min(4)).any(|k| is_header(unq[k]) && header_len(&unq[k..]) >= 3)
}

/// Splits plain text into what the sender wrote, the quoted history and any forwarded
/// email. `forward_subject` is whether the subject marks a forward.
pub fn split(text: &str, forward_subject: bool) -> Split {
    // Webmails that flatten HTML glue "-----Original Message-----" to the lines around it.
    static GLUED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)([^\n\s-])?[ \t]*(-{4,} ?(?:original message|message d'origine|mail original|message original|reply message) ?-{4,})[ \t]*([^\n\s-])?",
        )
        .expect("valid regex")
    });
    let unglued;
    let text = if text.contains("----") && GLUED.is_match(text) {
        unglued = GLUED.replace_all(text, |c: &regex::Captures| {
            let before = c
                .get(1)
                .map_or(String::new(), |m| format!("{}\n", m.as_str()));
            let after = c
                .get(3)
                .map_or(String::new(), |m| format!("\n{}", m.as_str()));
            format!("{before}{}{after}", &c[2])
        });
        unglued.as_ref()
    } else {
        text
    };
    // "Le 2 nov. 2026 à 08:27, Ana <a@x> a écrit :Coucou !": the quote glued to the
    // attribution that opens a line.
    static GLUED_ATTR: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?mi)^((?:le|on|am|el|il giorno|op) [^\n]{5,200}?(?:a écrit|wrote|schrieb|escribió|ha scritto|schreef) ?:)([^\s])",
        )
        .expect("valid regex")
    });
    let reglued;
    let text = if GLUED_ATTR.is_match(text) {
        reglued = GLUED_ATTR.replace_all(text, "$1\n$2");
        reglued.as_ref()
    } else {
        text
    };
    let lines: Vec<&str> = text.lines().collect();
    let Some((mut cut, start, forward)) = quote_cut(&lines, forward_subject) else {
        return Split {
            fresh: text.trim().to_string(),
            ..Default::default()
        };
    };
    if forward {
        // A header block is the forwarded email's own; a marker or separator line is not.
        let from = if is_forward_marker(lines[cut]) || is_original_separator(lines[cut]) {
            start
        } else {
            cut
        };
        return Split {
            fresh: joined(&lines[..cut]),
            quoted: String::new(),
            forward: Some(parse_forward(&lines[from..])),
        };
    }
    // A forward under an attribution is history like any quote.
    let quote_lines = &lines[start.min(lines.len())..];
    // Drop the separator rules and blank lines that sat just above the quote.
    while cut > 0 && (lines[cut - 1].trim().is_empty() || is_rule(lines[cut - 1])) {
        cut -= 1;
    }
    if cut == 0 {
        return bottom_posted(&lines, start);
    }
    let quoted_end = quote_lines
        .iter()
        .rposition(|l| is_quoted(l))
        .map_or(quote_lines.len(), |k| k + 1);
    Split {
        fresh: joined(&lines[..cut]),
        quoted: unquote(&quote_lines[..quoted_end]).trim().to_string(),
        forward: None,
    }
}

/// A message that opens on its quote: the reply sits under it (bottom posting) or
/// between its lines (inline reply), or it adds nothing.
fn bottom_posted(lines: &[&str], start: usize) -> Split {
    let rest = &lines[start.min(lines.len())..];
    let marked = rest.iter().any(|l| is_quoted(l));
    let last_quoted = rest.iter().rposition(|l| is_quoted(l));
    let unquoted: Vec<usize> = (0..rest.len())
        .filter(|&k| !rest[k].trim().is_empty() && !is_quoted(rest[k]))
        .collect();
    match (marked, last_quoted, unquoted.first()) {
        // Every line is quoted: nothing of its own.
        (true, _, None) => Split {
            fresh: String::new(),
            quoted: unquote(rest).trim().to_string(),
            forward: None,
        },
        // The reply under the whole quote.
        (true, Some(lq), Some(&first)) if first > lq => Split {
            fresh: joined(&rest[lq + 1..]),
            quoted: unquote(&rest[..=lq]).trim().to_string(),
            forward: None,
        },
        // Answers between quoted lines: read together, the attribution aside.
        (true, _, Some(_)) => Split {
            fresh: joined(rest),
            quoted: String::new(),
            forward: None,
        },
        // An attribution with unmarked text under it (clients that drop the `>`):
        // all of it is the history.
        (false, _, _) => Split {
            fresh: String::new(),
            quoted: joined(rest),
            forward: None,
        },
    }
}

/// Mobile footers and boilerplate that add nothing to a conversation.
const FOOTERS: &[&str] = &[
    "sent from my ",
    "sent from outlook",
    "sent from mail for windows",
    "sent from yahoo mail",
    "sent from samsung",
    "sent with proton mail",
    "sent via ",
    "envoyé de mon ",
    "envoyé depuis mon ",
    "envoyé depuis l'application",
    "envoyé depuis l’application",
    "envoyé à partir de ",
    "envoyé avec ",
    "ce message a été envoyé depuis un terminal",
    "envoyé depuis un ",
    "get outlook for ",
    "obtenir outlook pour ",
    "télécharger outlook",
    "téléchargez outlook",
    "von meinem iphone gesendet",
    "von meinem ipad gesendet",
    "inviato da ",
    "enviado desde mi ",
];
const DISCLAIMERS: &[&str] = &[
    "this email and any attachments",
    "this e-mail and any attachments",
    "this message is confidential",
    "this email is confidential",
    "this e-mail is confidential",
    "this message and any attachments",
    "the information contained in this",
    "confidentiality notice",
    "disclaimer:",
    "ce message et toutes les pièces jointes",
    "ce message et ses pièces jointes",
    "ce courriel et toutes les pièces jointes",
    "ce message est confidentiel",
    "ce courriel est confidentiel",
    "les informations contenues dans ce",
    "le présent message",
    "avertissement :",
    "avertissement:",
    "pensez à l'environnement",
    "pensez à l’environnement",
    "pensez à protéger notre planète",
    "please consider the environment",
    "think before you print",
    "l'absence de virus dans ce courrier",
    "l’absence de virus dans ce courrier",
    "this email has been checked for viruses",
    "this email has been scanned",
];
/// Closing words a sign-off name follows ("Merci,\nNora").
const CLOSINGS: &[&str] = &[
    "merci",
    "cordialement",
    "bien à vous",
    "bien à toi",
    "bien cordialement",
    "bonne journée",
    "bonne soirée",
    "à bientôt",
    "a bientôt",
    "amicalement",
    "bises",
    "salutations",
    "best",
    "regards",
    "kind regards",
    "best regards",
    "warm regards",
    "cheers",
    "thanks",
    "thank you",
    "sincerely",
    "sincèrement",
    "bien sincèrement",
    "très cordialement",
    "respectueusement",
    "bonne fin de journée",
    "belle journée",
    "excellente journée",
    "merci d'avance",
    "à plus",
    "a+",
    "@+",
    "a +",
    "talk soon",
    "see you",
];

/// A line that only closes a letter ("Merci,", "Best regards").
fn is_closing(line: &str) -> bool {
    let t = line
        .trim()
        .trim_end_matches([',', '.', '!'])
        .trim()
        .to_lowercase();
    !t.is_empty() && t.chars().count() <= 40 && CLOSINGS.iter().any(|c| t.starts_with(c))
}

/// Removes the "-- " signature block, mobile footers, trailing legal disclaimers and
/// contact blocks. Returns (what is left, whether anything was removed); what is left
/// is empty when the text was only a signature.
pub fn strip_signature(text: &str) -> (String, bool) {
    let lines: Vec<&str> = text.lines().collect();
    let mut cut = lines.len();
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        // RFC 3676 signature delimiter, or a footer/disclaimer starting a line.
        let delimiter = t == "--" || t == "—" || t == "-- --";
        let footer = t.chars().count() <= 120 && starts_ci(t, FOOTERS);
        let disclaimer = i > 0 && starts_ci(t, DISCLAIMERS);
        if delimiter || footer || disclaimer {
            cut = i;
            break;
        }
    }
    if let Some(start) = contact_block(&lines[..cut]) {
        cut = start;
    }
    if cut == lines.len() {
        return (text.trim().to_string(), false);
    }
    (lines[..cut].join("\n").trim().to_string(), true)
}

/// A line of contact details: an address, a phone number or a web page on its own.
fn is_contact_line(line: &str) -> bool {
    let t = line.trim().trim_matches(['|', ' ']);
    // "[cid:image002.jpg@01D9…]" is a picture, not an address.
    if t.starts_with("[cid:") {
        return false;
    }
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
    // "75205 Paris Cedex 13, France", "Mailing address:": a postal address.
    let postal = t.len() <= 60
        && (t
            .split_whitespace()
            .next()
            .is_some_and(|w| w.len() == 5 && w.chars().all(|c| c.is_ascii_digit()))
            || low.contains("cedex")
            || low.starts_with("mailing address"));
    email || web || phone || postal
}

/// A short line a signature carries: a name, a title, a company.
fn is_name_line(line: &str) -> bool {
    let t = line.trim();
    let words = t.split_whitespace().count();
    // A job title runs long: "Élève Ingénieur Civil à l'École des Mines - Mines Paris".
    // Names, titles and addresses start with a capital or a digit; a sentence that
    // happens to lack its full stop ("voici la version corrigée") does not.
    let starts = t.chars().next().is_some_and(|c| !c.is_lowercase());
    starts
        && (1..=12).contains(&words)
        && t.chars().count() <= 90
        && !t.ends_with([',', '.', '!', '?', ':', ';'])
}

/// Where a trailing block of contact details starts ("Name / email / +33 phone"): at
/// least two lines, one of them an address, phone or web page. A name right under a
/// closing ("Merci,\nNora") is the sign-off and stays.
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
        if !contact && !is_name_line(l) || is_closing(l) {
            break;
        }
        strong += contact as usize;
        kept += 1;
        start = i;
        if kept >= 7 {
            break;
        }
    }
    // A signature flattened onto one line ("Ana Perezana@x.example+33 6 11 22 33 44")
    // carries an address and a phone or profile on its own.
    let dense = |l: &str| {
        let low = l.to_lowercase();
        l.contains('@')
            && (low.contains("linkedin.") || l.chars().filter(|c| c.is_ascii_digit()).count() >= 8)
    };
    if (kept < 2 || strong < 1) && !(kept == 1 && dense(lines[start])) {
        return None;
    }
    let above = lines[..start].iter().rposition(|l| !l.trim().is_empty());
    let named = is_name_line(lines[start]) && !is_contact_line(lines[start]);
    match above {
        // The sign-off name under a closing stays.
        Some(k) if is_closing(lines[k]) && named => (start + 1 < lines.len()).then_some(start + 1),
        Some(_) => Some(start),
        // Nothing above: with several addresses or phones it is only a signature (the
        // reader shows its first line when nothing else is left); with one, its first
        // line is what was written ("Junior est au fond du seau", then "Fred", a phone).
        None if strong >= 2 => Some(start),
        None => (start + 1 < lines.len()).then_some(start + 1),
    }
}

/// What a reader needs: new content without quoted history, signature or boilerplate.
/// Returns (content, whether anything was hidden).
pub fn readable(text: &str) -> (String, bool) {
    let r = crate::reading::read(&crate::reading::Mail {
        text,
        ..Default::default()
    });
    (r.text, r.has_quoted || r.has_signature)
}

/// The list preview of a message: its new text on one line.
pub fn snippet(body: &str) -> String {
    let r = crate::reading::read(&crate::reading::Mail {
        text: body,
        ..Default::default()
    });
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
    r.text
        .lines()
        .filter(|l| !looks_like_css(l))
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
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
    "class=\"protonmail_quote\"",
    "data-marker=\"__quoted_text__\"",
];

/// Whether a text names a quoted email somewhere: an attribution or a separator, as a
/// plain part flattened from HTML still does.
pub fn mentions_attribution(text: &str) -> bool {
    let low = text.to_lowercase().replace(" :", ":");
    ATTR_END.iter().any(|e| low.contains(e)) || ORIGINAL.iter().any(|o| low.contains(o))
}

/// Whether an HTML body marks quoted history.
pub fn has_html_quote(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    HTML_MARKERS.iter().any(|m| lower.contains(m))
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

    #[rstest]
    #[case::exact("from: a", HEADER_FROM, true)]
    #[case::upper("FROM: a", HEADER_FROM, true)]
    #[case::indented("   Von: a", HEADER_FROM, true)]
    #[case::accented_upper("À : b", HEADER_REST, true)]
    #[case::french_space("Objet : b", HEADER_REST, true)]
    #[case::bold("*From:* Nora", HEADER_FROM, true)]
    #[case::other("Hello", HEADER_FROM, false)]
    #[case::blank("   ", HEADER_FROM, false)]
    #[case::shorter_than_prefix("fro", HEADER_FROM, false)]
    fn case_insensitive_prefixes(#[case] line: &str, #[case] set: &[&str], #[case] want: bool) {
        assert_eq!(starts_ci(line, set), want);
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
    #[case::wrapped_in_two("On Sun, Oct 11, 2026 at 8:12 AM Marc <m@x.example>\nwrote:", Some(2))]
    #[case::one_line("Le dim. 11 oct. 2026 à 08:12, Marc <m@x.example> a écrit :", Some(1))]
    #[case::without_opener("Marc Petit <m@x.example> a écrit :", Some(1))]
    #[case::italian(
        "Il giorno 14 apr 2026, alle ore 20:18, Lea <l@x.example> ha scritto:",
        Some(1)
    )]
    #[case::horde("Quoting Marc Petit <m@x.example>:", Some(1))]
    #[case::prose_wrote("She wrote: a poem.", None)]
    #[case::prose_le("Le chat dort.", None)]
    fn attributions(#[case] text: &str, #[case] want: Option<usize>) {
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(attribution(&lines, 0), want);
    }

    #[rstest]
    #[case::rfc_delimiter("See you.\n-- \nMarc Petit\nCFO", "See you.")]
    #[case::iphone_footer("Ok!\n\nSent from my iPhone", "Ok!")]
    #[case::french_footer("D'accord\n\nEnvoyé de mon iPhone", "D'accord")]
    #[case::orange_footer("Oui\n\nEnvoyé depuis l'application Mail Orange", "Oui")]
    #[case::disclaimer("Done.\n\nThis email and any attachments are confidential.", "Done.")]
    #[case::plain_kept("Line one\n--\n", "Line one")]
    #[case::name_email_phone(
        "Here is the plan.\n\nNora Lind\nnora@agency.example\n+33 6 12 34 56 78",
        "Here is the plan."
    )]
    #[case::labelled_phone(
        "See you then.\n\nNora Lind\nAgence Lind\nTél : +33 6 12 34 56 78",
        "See you then.\n\nNora Lind"
    )]
    #[case::sign_off_name_kept(
        "Thanks for the quick answer.\n\nMerci,\nNora Lind\nnora@agency.example",
        "Thanks for the quick answer.\n\nMerci,\nNora Lind"
    )]
    #[case::website(
        "Draft attached.\n\nSam Ito\nStudio Ito\nwww.studio.example",
        "Draft attached."
    )]
    #[case::dash_delimiter("Talk soon.\n--\nSam", "Talk soon.")]
    fn signatures_are_stripped(#[case] text: &str, #[case] want: &str) {
        assert_eq!(strip_signature(text).0, want);
    }

    #[rstest]
    fn a_message_that_is_only_a_signature_leaves_nothing() {
        assert_eq!(strip_signature("-- \nMarc").0, "");
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
