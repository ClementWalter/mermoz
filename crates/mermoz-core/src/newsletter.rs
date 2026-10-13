//! Readable text for bulk mail (newsletters, promos, notices), done locally. Their plain
//! part is often a degraded dump: a "view in browser" tracking link split over lines,
//! bracketed image names, unsubscribe and legal footers. This module turns such mail
//! into what a reader sees first: its headline and sentences. The full original
//! (HTML, links) stays untouched for the expanded email.

/// Redirect and tracking hosts of mailing services: their links are never content.
const TRACKERS: &[&str] = &[
    "list-manage.com",
    "mailchi.mp",
    "mcusercontent.com",
    "sendgrid.net",
    "hubspotlinks.com",
    "hs-sites.com",
    "hubspotemail.net",
    "sendinblue.com",
    "brevo.com",
    "r.mail",
    "substack.com/redirect",
    "email.mg.",
    "mailjet.com",
    "mjt.lu",
    "ct.sendgrid",
    "click.",
    "trk.",
    "track.",
    "links.",
    "clicks.",
    "e.customeriomail",
    "cmail19.com",
    "cmail20.com",
    "createsend",
    "exacttarget",
    "/track/click",
    "utm_",
];

/// Lines that only point to another way of reading the same mail.
const VIEW_ONLINE: &[&str] = &[
    "view in browser",
    "view this email in your browser",
    "view it in your browser",
    "view email in browser",
    "view online",
    "open in browser",
    "open in your browser",
    "ouvrir dans le navigateur",
    "ouvrir dans votre navigateur",
    "web version",
    "having trouble viewing",
    "can't see this email",
    "voir la version en ligne",
    "version en ligne",
    "afficher dans le navigateur",
    "afficher dans votre navigateur",
    "voir dans le navigateur",
    "voir dans votre navigateur",
    "si ce message ne s'affiche pas",
    "si cet e-mail ne s'affiche pas",
    "si cet email ne s'affiche pas",
    "cliquez ici pour le visualiser",
];

/// Lines that open the footer: everything from there on is boilerplate.
const FOOTER: &[&str] = &[
    "unsubscribe",
    "update your preferences",
    "manage your preferences",
    "manage preferences",
    "you are receiving this",
    "you received this email",
    "why did i get this",
    "se désabonner",
    "désabonnez",
    "désinscri",
    "désabonnement",
    "gérer vos préférences",
    "gérer mes préférences",
    "vous recevez cet e-mail",
    "vous recevez cet email",
    "vous recevez ce message",
    "pour ne plus recevoir",
    "all rights reserved",
    "tous droits réservés",
    "mentions légales",
    "privacy policy",
    "politique de confidentialité",
    "copyright ©",
    "© 20",
];

const INVISIBLE: &[char] = &[
    '\u{200B}', '\u{200C}', '\u{200D}', '\u{2060}', '\u{FEFF}', '\u{034F}', '\u{00AD}', '\u{180E}',
    '\u{00A0}',
];

fn is_tracker(url: &str) -> bool {
    let u = url.to_lowercase();
    TRACKERS.iter().any(|t| u.contains(t))
}

fn is_url(token: &str) -> bool {
    // Every word of every message is asked: reject on the first byte when it can't start one.
    if !matches!(
        token.as_bytes().first(),
        Some(b'h' | b'w' | b'<' | b'(' | b'[' | b'"' | b'\'')
    ) {
        return false;
    }
    let t = token.trim_matches(|c| "<>()[]\"'".contains(c));
    t.starts_with("http://") || t.starts_with("https://") || t.starts_with("www.")
}

/// A piece of a URL that wrapped onto its own line: no spaces, query-like characters.
fn is_url_tail(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty()
        && !t.contains(' ')
        && t.len() >= 12
        && t.chars().any(|c| "=&%?/_".contains(c))
        && t.chars().all(|c| c.is_ascii_graphic())
}

/// A line made only of a link (a tracking link, or any long one): read in the full email.
fn is_link_line(line: &str) -> bool {
    let t = line
        .trim()
        .trim_start_matches(['(', '[', '<'])
        .trim_end_matches([')', ']', '>']);
    !t.contains(' ') && is_url(t) && (is_tracker(t) || t.len() > 40)
}

/// "[image: Logo]", "[Logo]", "(image)", "** ": names of pictures and markdown debris.
fn is_image_noise(line: &str) -> bool {
    let t = line.trim();
    let low = t.to_lowercase();
    (t.starts_with('[') && t.ends_with(']') && t.chars().count() <= 60)
        || low.starts_with("[image")
        || low == "(image)"
        || t.chars().all(|c| "*-=_~#|·•".contains(c)) && !t.is_empty()
}

/// Drops tracking links inside a sentence; other links stay, they are content.
fn without_tracking_links(line: &str) -> String {
    let mut out: Vec<String> = vec![];
    for word in line.split(' ') {
        if is_url(word) && is_tracker(word) {
            continue;
        }
        out.push(word.to_string());
    }
    // "Read more ()" once its link is gone.
    out.join(" ")
        .replace(" ()", "")
        .replace("( )", "")
        .replace("[]", "")
        .trim_end()
        .to_string()
}

/// The readable part of a bulk mail's text: preamble links, tracking links, image
/// names and the footer removed, blank runs folded, starting at its first real line.
pub fn clean(text: &str) -> String {
    let text = tidy(&plain_markdown(text));
    let text = text.as_str();
    let text: String = text
        .chars()
        .filter(|c| !INVISIBLE.contains(c) || *c == '\u{00A0}')
        .collect();
    let text = text.replace('\u{00A0}', " ");
    let lines: Vec<&str> = text.lines().collect();
    let content_lines = lines.iter().filter(|l| !l.trim().is_empty()).count();
    let mut out: Vec<String> = vec![];
    let mut after_link = false;
    let mut seen = 0usize;
    for line in &lines {
        let low = line.trim().to_lowercase();
        if !low.is_empty() {
            seen += 1;
        }
        // The footer starts late in the mail, after some content.
        let late = seen * 3 >= content_lines.max(1) && out.iter().any(|l| !l.trim().is_empty());
        if late && FOOTER.iter().any(|f| low.contains(f)) {
            break;
        }
        if VIEW_ONLINE.iter().any(|v| low.contains(v)) && low.len() < 160 {
            after_link = true;
            continue;
        }
        // The rest of a link that wrapped, down to its closing parenthesis.
        let tail = after_link
            && !line.trim().is_empty()
            && !line.trim().contains(' ')
            && (is_url_tail(line) || line.trim().ends_with(')'));
        if is_link_line(line) || tail {
            after_link = true;
            continue;
        }
        after_link = false;
        if is_image_noise(line) {
            continue;
        }
        let kept = without_tracking_links(line.trim_end());
        let kept = kept
            .trim_start_matches("** ")
            .trim_start_matches("# ")
            .to_string();
        if kept.trim().is_empty() && !line.trim().is_empty() {
            continue;
        }
        out.push(kept);
    }
    let mut folded: Vec<String> = vec![];
    for l in out {
        let blank = l.trim().is_empty();
        if blank && folded.last().is_none_or(|p: &String| p.trim().is_empty()) {
            continue;
        }
        folded.push(l);
    }
    folded.join("\n").trim().to_string()
}

/// Whether a "plain" part is really HTML source (some senders put the page itself in
/// text/plain): it opens on a doctype or `<html>`, or tags make up much of it.
pub fn looks_like_html(text: &str) -> bool {
    let head: String = text
        .trim_start_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
        .chars()
        .take(20)
        .collect::<String>()
        .to_ascii_lowercase();
    if head.starts_with("<!doctype") || head.starts_with("<html") || head.starts_with("<?xml") {
        return true;
    }
    // Cheap first: prose with a stray "<br>" never gets the full count.
    if !text.contains("</") {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    const TAGS: &[&str] = &[
        "<div", "</div", "<td", "</td", "<tr", "</tr", "<table", "<span", "</span", "<p>", "<p ",
        "</p>", "<br", "<a ", "</a>", "<img", "<meta", "<style", "<font", "<center",
    ];
    let tags: usize = TAGS.iter().map(|t| lower.matches(t).count()).sum();
    let words = text.split_whitespace().count().max(1);
    tags >= 8 && tags * 6 >= words
}

/// Whether a plain part is a degraded dump of the HTML: empty, opening on a link, or
/// mostly links.
pub fn degraded(text: &str) -> bool {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if is_url(first.trim()) {
        return true;
    }
    // A "view online" banner opens the line: a long unwrapped body is not searched whole.
    let low = first.chars().take(200).collect::<String>().to_lowercase();
    if VIEW_ONLINE.iter().any(|v| low.contains(v)) {
        return true;
    }
    // Without a link anywhere, only an empty text is degraded: no word walk needed.
    if !text.contains("http") && !text.contains("www.") {
        return text.trim().is_empty();
    }
    // One pass, no allocation: it runs on every message with HTML that is opened.
    let (mut chars, mut links, mut words) = (0usize, 0usize, 0usize);
    for w in text.split_whitespace() {
        words += 1;
        chars += w.len();
        if is_url(w) {
            links += w.len();
        }
    }
    words == 0 || links * 10 > chars.max(1) * 3
}

/// Readable text of an HTML body: headings, paragraphs and list items on their own
/// lines, links as their words, pictures, styles and scripts left out.
pub fn html_text(html: &str) -> String {
    // The head (title, preloads, the doctype's address) is never text.
    let html = match html.to_ascii_lowercase().find("</head>") {
        Some(end) => &html[end + "</head>".len()..],
        None => html,
    };
    // Block ends become line breaks before the tags are dropped.
    let mut marked = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    const BREAK: &[&str] = &[
        "<br",
        "</p",
        "<p ",
        "<p>",
        "</div",
        "</h1",
        "</h2",
        "</h3",
        "</h4",
        "</h5",
        "</h6",
        "</tr",
        "</table",
        "</ul",
        "</ol",
        "</blockquote",
    ];
    while i < html.len() {
        let rest = &lower[i..];
        if rest.starts_with("<li") {
            marked.push_str("\n• ");
        } else if BREAK.iter().any(|b| rest.starts_with(b)) {
            marked.push('\n');
        }
        let ch = html[i..].chars().next().unwrap();
        marked.push(ch);
        i += ch.len_utf8();
    }
    let text = ammonia::Builder::default()
        .tags(std::collections::HashSet::new())
        .clean(&marked)
        .to_string();
    let text = text
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    // Escaped markup in the page (templates, quoted source) decodes back into tags.
    let text = without_tags(&text);
    text.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// HTML element names a text never means literally.
const TAG_NAMES: &[&str] = &[
    "a", "abbr", "b", "body", "br", "center", "div", "em", "font", "h1", "h2", "h3", "h4", "h5",
    "h6", "head", "hr", "html", "i", "img", "li", "link", "meta", "ol", "p", "span", "strong",
    "style", "table", "tbody", "td", "th", "thead", "tr", "u", "ul", "!doctype",
];

/// `text` with HTML tags (`<p>`, `</div>`, `<a href=…>`) removed; a `<br>` or a block
/// end becomes a line break. Other angle brackets ("<3", "a < b", "<x@y.z>") stay.
pub fn without_tags(text: &str) -> String {
    if !text.contains('<') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let name: String = after
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '!')
            .collect::<String>()
            .to_ascii_lowercase();
        let close = after.find('>');
        match close {
            Some(c) if TAG_NAMES.contains(&name.as_str()) && !after[..c].contains('<') => {
                if matches!(name.as_str(), "br" | "p" | "div" | "li" | "tr") && !out.ends_with('\n')
                {
                    out.push('\n');
                }
                rest = &after[c + 1..];
            }
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// What a bulk mail shows in a chat bubble: its HTML read as text when the plain part
/// is a degraded dump (or missing), else its plain part, both cleaned.
pub fn readable(plain: &str, html: Option<&str>) -> String {
    let from_plain = clean(plain);
    match html {
        Some(h)
            if plain.trim().is_empty()
                || degraded(plain)
                || looks_like_html(plain)
                || from_plain.is_empty() =>
        {
            let from_html = clean(&html_text(h));
            if from_html.is_empty() {
                from_plain
            } else {
                from_html
            }
        }
        _ => from_plain,
    }
}

/// Index of the `)` closing a `(` at `open`, if the parenthesis holds a link.
fn link_end(chars: &[char], open: usize) -> Option<usize> {
    if chars.get(open) != Some(&'(') {
        return None;
    }
    let mut depth = 0;
    for (k, c) in chars.iter().enumerate().skip(open).take(2000) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            ' ' if depth == 1 && k > open + 1 && !chars[open + 1..k].contains(&'"') => {
                return None;
            }
            _ => {}
        }
    }
    None
}

/// Index of the `]` closing a `[` at `open` (nested brackets allowed, a few lines long).
fn bracket_end(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (k, c) in chars.iter().enumerate().skip(open).take(600) {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

/// Plain words from Markdown-like text some senders put in their plain part (and the
/// AI's clean version uses): images dropped, `[text](url)` read as "text", rules,
/// heading marks and emphasis marks removed.
pub fn plain_markdown(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        // ![alt](url): a picture, never text.
        if chars[i] == '!'
            && chars.get(i + 1) == Some(&'[')
            && let Some(close) = bracket_end(&chars, i + 1)
            && let Some(end) = link_end(&chars, close + 1)
        {
            i = end + 1;
            continue;
        }
        // [text](url): its words; the inner text is read the same way (it may hold an image).
        if chars[i] == '['
            && let Some(close) = bracket_end(&chars, i)
            && let Some(end) = link_end(&chars, close + 1)
        {
            let inner: String = chars[i + 1..close].iter().collect();
            out.push_str(plain_markdown(&inner).trim());
            i = end + 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    let mut folded: Vec<String> = vec![];
    for l in out.lines() {
        let t = l.trim();
        // Horizontal rules: "---", "***", "___", "- - -".
        if t.len() >= 3 && t.chars().all(|c| "-*_= ".contains(c)) {
            continue;
        }
        let l = if t.starts_with('#') || t.starts_with("> ") {
            t.trim_start_matches(['#', '>']).trim_start().to_string()
        } else {
            l.to_string()
        };
        let l = l.replace("**", "").replace("__", "");
        if l.trim().is_empty() && folded.last().is_none_or(|p: &String| p.trim().is_empty()) {
            continue;
        }
        folded.push(l.trim_end().to_string());
    }
    folded.join("\n").trim().to_string()
}

/// Characters that take no room: a line made of them (and spaces) is blank.
const ZERO_WIDTH: &[char] = &[
    '\u{200B}', '\u{200C}', '\u{200D}', '\u{200E}', '\u{200F}', '\u{2060}', '\u{2061}', '\u{2062}',
    '\u{2063}', '\u{2064}', '\u{FEFF}', '\u{034F}', '\u{00AD}', '\u{180E}', '\u{FFFC}',
];

/// Picture names that say nothing: dropped rather than kept as a line of text.
const GENERIC_ALT: &[&str] = &[
    "logo",
    "image",
    "img",
    "header",
    "banner",
    "bandeau",
    "spacer",
    "icon",
    "icône",
    "photo",
    "picture",
    "pixel",
    "separator",
    "séparateur",
    "facebook",
    "twitter",
    "instagram",
    "linkedin",
    "youtube",
    "x",
    "email header",
    "en-tête",
];

/// A picture's alt text worth reading (a headline), not a generic name.
fn meaningful_alt(alt: &str) -> bool {
    let a = alt.trim().to_lowercase();
    a.split_whitespace().count() >= 2
        && !GENERIC_ALT.contains(&a.as_str())
        && !a.split_whitespace().all(|w| GENERIC_ALT.contains(&w))
}

/// "[https://…]" on its own: a picture or link address in brackets.
fn bracketed_url(token: &str) -> bool {
    let t = token.trim();
    t.starts_with('[')
        && t.ends_with(']')
        && !t[1..t.len() - 1].contains(' ')
        && is_url(&t[1..t.len() - 1])
}

/// Mailjet / Brevo picture placeholders: `<<alt>> [https://…img…]` reads as the alt
/// text when it is a headline, and disappears otherwise; bracketed addresses go.
fn without_placeholders(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(open) = rest.find("<<") {
        let Some(close) = rest[open..].find(">>").map(|c| open + c) else {
            break;
        };
        out.push_str(&rest[..open]);
        let alt = &rest[open + 2..close];
        if meaningful_alt(alt) {
            out.push_str(alt.trim());
        }
        rest = &rest[close + 2..];
    }
    out.push_str(rest);
    out.split(' ')
        .filter(|w| !bracketed_url(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A text ready for a bubble: zero-width characters and picture placeholders gone, lines
/// of only (Unicode) spaces counted as blank, blank runs folded, no blank first or last
/// line.
pub fn tidy(text: &str) -> String {
    let mut out: Vec<String> = vec![];
    for line in text.lines() {
        let l: String = line.chars().filter(|c| !ZERO_WIDTH.contains(c)).collect();
        let l = if l.contains("<<") || l.contains("[http") {
            without_placeholders(&l)
        } else {
            l
        };
        if l.trim().is_empty() {
            if out.last().is_some_and(|p| !p.is_empty()) {
                out.push(String::new());
            }
        } else {
            out.push(l.trim_end().to_string());
        }
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const MAILCHIMP: &str = "View this email in your browser (https://us.list-manage.com/AIW_vCP29mP?e=4f2a\n9c&c2id=1b2c3d4e5f60718293a4b5c6\nd7e8f9&m=2f3e4d5c6b7a\n8091)\n\n\n[image: Lettre du Quartier]\n\n** 42% des habitants du quartier utilisent le vélo chaque semaine\n------------------------------------------------------------\nLa mairie lance une consultation sur les pistes cyclables. Donnez votre avis (https://us.list-manage.com/track/click?u=1&id=2) avant le 30.\n\nNotre rapport complet est sur https://quartier.example/rapport-velo.\n\n============================================================\nVous recevez cet email car vous êtes inscrit à la lettre du quartier.\nSe désabonner (https://us.list-manage.com/unsubscribe?u=1)\nCopyright © 2026 Lettre du Quartier, Tous droits réservés.";

    const SUBSTACK: &str = "View in browser: https://open.substack.com/pub/notes/p/the-week?utm_source=email&utm_medium=email\n\nThe Week in Notes\n\nThree things worth reading this week, starting with a long piece on city gardens.\n\nRead more https://substack.com/redirect/2/eyJ1IjoiaHR0cHM6Ly9ub3Rlcy5leGFtcGxl\n\nUnsubscribe https://substack.com/redirect/2/unsubscribe\n© 2026 Notes";

    const PROMO: &str = "Si cet e-mail ne s'affiche pas correctement, cliquez ici pour le visualiser.\nhttps://click.maison-co.example/v/0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c\n\n[Maison & Co]\n\nJusqu'à -30% sur toute la literie ce week-end\n\nProfitez de nos prix doux sur les couettes, oreillers et parures.\n\nPour ne plus recevoir nos offres, cliquez ici.\nMentions légales : Maison & Co SAS, 12 rue des Lilas, 75011 Paris.";

    const BANK: &str = "Afficher dans le navigateur\n\nDemo Bank\n\nVotre relevé d'octobre est disponible dans votre espace client.\n\nCe message est automatique, merci de ne pas y répondre.\n© 2026 Demo Bank - Tous droits réservés";

    fn first_line(s: &str) -> &str {
        s.lines().next().unwrap_or("")
    }

    #[rstest]
    #[case::mailchimp(
        MAILCHIMP,
        "42% des habitants du quartier utilisent le vélo chaque semaine"
    )]
    #[case::substack(SUBSTACK, "The Week in Notes")]
    #[case::promo(PROMO, "Jusqu'à -30% sur toute la literie ce week-end")]
    #[case::bank(BANK, "Demo Bank")]
    fn the_first_line_is_real_content(#[case] text: &str, #[case] want: &str) {
        assert_eq!(first_line(&clean(text)), want);
    }

    #[rstest]
    #[case::mailchimp(MAILCHIMP)]
    #[case::substack(SUBSTACK)]
    #[case::promo(PROMO)]
    #[case::bank(BANK)]
    fn no_tracking_link_is_left(#[case] text: &str) {
        let out = clean(text);
        assert!(
            !["list-manage", "substack.com/redirect", "click.maison"]
                .iter()
                .any(|t| out.contains(t))
        );
    }

    #[rstest]
    #[case::mailchimp(MAILCHIMP)]
    #[case::substack(SUBSTACK)]
    #[case::promo(PROMO)]
    #[case::bank(BANK)]
    fn no_view_in_browser_line_is_left(#[case] text: &str) {
        let out = clean(text).to_lowercase();
        assert!(!VIEW_ONLINE.iter().any(|v| out.contains(v)));
    }

    #[rstest]
    #[case::mailchimp(MAILCHIMP, "désabonner")]
    #[case::substack(SUBSTACK, "Unsubscribe")]
    #[case::promo(PROMO, "Mentions légales")]
    #[case::bank(BANK, "droits réservés")]
    fn footers_are_dropped(#[case] text: &str, #[case] footer: &str) {
        assert!(!clean(text).contains(footer));
    }

    #[rstest]
    fn content_links_in_sentences_are_kept() {
        assert!(clean(MAILCHIMP).contains("https://quartier.example/rapport-velo"));
    }

    #[rstest]
    fn a_sentence_keeps_its_words_without_its_tracking_link() {
        assert!(clean(MAILCHIMP).contains("Donnez votre avis avant le 30."));
    }

    #[rstest]
    fn image_names_are_dropped() {
        assert!(!clean(MAILCHIMP).contains("[image"));
    }

    #[rstest]
    fn blank_runs_fold_to_one() {
        assert!(!clean("Title\n\n\n\nText").contains("\n\n\n"));
    }

    #[rstest]
    fn a_short_link_alone_on_a_line_stays() {
        assert_eq!(
            clean("Here it is:\nhttps://a.example/x"),
            "Here it is:\nhttps://a.example/x"
        );
    }

    #[rstest]
    fn an_early_mention_of_unsubscribing_is_not_a_footer() {
        assert!(
            clean("Unsubscribe is now one click.\nWe changed how it works.\nMore below.")
                .starts_with("Unsubscribe")
        );
    }

    #[rstest]
    #[case::opens_on_a_link(MAILCHIMP, true)]
    #[case::empty("  ", true)]
    #[case::mostly_links(
        "https://a.example/aaaaaaaaaaaaaaaaaaaaaaaaaaaaa https://b.example/bbbbbbbbbbbbbbbbbbbbb hi",
        true
    )]
    #[case::prose("Votre relevé d'octobre est disponible.", false)]
    #[case::bare_www_links(
        "www.a.example/aaaaaaaaaaaaaaaaaaaaaaaaaaaaa www.b.example/bbbbbbbbbbbbbbbbbbbbb hi",
        true
    )]
    #[case::banner_first("View this email in your browser\nHello", true)]
    #[case::banner_deep_in_one_long_line(concat!(
        "Hello there, here is a very long unwrapped paragraph that goes on and on about ",
        "the quarterly numbers, the team offsite, the new office plants and the coffee ",
        "machine that finally works again after months of trouble; anyway, view online"
    ), false)]
    #[case::prose_with_one_link(
        "Hi team, please read the minutes at https://notes.example/q3 before Friday's \
         meeting so we can decide on the budget and the hiring plan together. Thanks!",
        false
    )]
    fn degraded_plain_parts(#[case] text: &str, #[case] want: bool) {
        assert_eq!(degraded(text), want);
    }

    const HTML: &str = "<html><head><style>p{color:red}</style></head><body><div style=\"display:none\">\u{200C}\u{200C}</div><p><a href=\"https://us.list-manage.com/view?u=1\">Voir la version en ligne</a></p><img src=\"cid:logo\" alt=\"Logo\"><h1>Le marché rouvre samedi</h1><p>Venez d&eacute;couvrir les <b>producteurs</b> &amp; artisans.</p><ul><li>Fromages</li><li>Pain</li></ul><p>Se d&eacute;sabonner</p></body></html>";

    #[rstest]
    fn html_reads_from_its_headline() {
        assert_eq!(
            first_line(&readable("https://us.list-manage.com/view?u=1", Some(HTML))),
            "Le marché rouvre samedi"
        );
    }

    #[rstest]
    fn html_lists_read_as_bullets() {
        assert!(readable("", Some(HTML)).contains("• Fromages\n• Pain"));
    }

    #[rstest]
    fn html_styles_are_not_text() {
        assert!(!readable("", Some(HTML)).contains("color:red"));
    }

    #[rstest]
    fn a_good_plain_part_is_kept() {
        assert_eq!(
            readable("Votre relevé est disponible.", Some(HTML)),
            "Votre relevé est disponible."
        );
    }

    const SPORTS: &str = "![Email header](https://static.club.example/emails/email-header.jpg)\n\n---\n\n[ ![](https://media2.club.example/team/42/logo/abc.png) ](https://club.example/team/42)\n\n## Match ce samedi à 10h\n\nBonjour, **le match contre Les Aigles** est confirmé. [Répondre à la convocation](https://club.example/r/123) avant jeudi.\n\n---\n\nÀ samedi !";

    #[rstest]
    fn markdown_images_are_dropped() {
        assert!(!plain_markdown(SPORTS).contains("!["));
    }

    #[rstest]
    fn markdown_links_read_as_their_words() {
        assert!(plain_markdown(SPORTS).contains("Répondre à la convocation avant jeudi."));
    }

    #[rstest]
    fn markdown_rules_are_dropped() {
        assert!(!plain_markdown(SPORTS).lines().any(|l| l.trim() == "---"));
    }

    #[rstest]
    #[case::link_syntax("](")]
    #[case::bold("**")]
    #[case::heading("## ")]
    #[case::image("![")]
    fn no_markdown_syntax_is_left(#[case] mark: &str) {
        assert!(!plain_markdown(SPORTS).contains(mark));
    }

    #[rstest]
    fn the_first_line_is_the_headline() {
        assert_eq!(
            plain_markdown(SPORTS).lines().next(),
            Some("Match ce samedi à 10h")
        );
    }

    #[rstest]
    fn the_sports_mail_reads_cleanly_in_a_bubble() {
        assert!(readable(SPORTS, None).starts_with("Match ce samedi à 10h"));
    }

    #[rstest]
    #[case::brackets_without_link("Prix [indicatif] : 10 €", "Prix [indicatif] : 10 €")]
    #[case::bare_link(
        "Voir https://club.example/r/1 ici",
        "Voir https://club.example/r/1 ici"
    )]
    #[case::dash_in_a_sentence("Score: 3 - 2", "Score: 3 - 2")]
    #[case::parenthesis_after_brackets("Voir [annexe] (page 3)", "Voir [annexe] (page 3)")]
    fn ordinary_text_is_untouched(#[case] text: &str, #[case] want: &str) {
        assert_eq!(plain_markdown(text), want);
    }

    /// A town newsletter sent through Mailjet: a portrait placeholder, lines of invisible
    /// characters and non-breaking spaces, then the letter.
    const TOWN: &str = "<<Camille Durand - Maire de Villeneuve, Vice-Présidente de l'agglomération>> [https://x1abc.mjt.lu/img/x1abc/b/portrait.png]\n\u{00A0}\n\u{200B}\u{200C}\n\u{2007}\u{FEFF}\nChers Habitants,\n\u{00A0}\nLe nouveau marché ouvre samedi place de la Mairie.\n \n<<Texte à la une>> [https://x1abc.mjt.lu/img/x1abc/b/une.png]\n<<logo>> [https://x1abc.mjt.lu/img/x1abc/b/logo.png]\n\u{00A0}\n";

    #[rstest]
    fn the_first_line_is_never_blank() {
        assert!(!tidy(TOWN).lines().next().unwrap().trim().is_empty());
    }

    #[rstest]
    fn a_headline_placeholder_reads_as_its_words() {
        assert_eq!(
            tidy(TOWN).lines().next(),
            Some("Camille Durand - Maire de Villeneuve, Vice-Présidente de l'agglomération")
        );
    }

    #[rstest]
    fn picture_addresses_are_gone() {
        assert!(!tidy(TOWN).contains("mjt.lu"));
    }

    #[rstest]
    fn a_generic_picture_name_is_dropped() {
        assert!(!tidy(TOWN).contains("logo"));
    }

    #[rstest]
    fn invisible_lines_fold_into_one_blank() {
        assert_eq!(
            tidy(TOWN),
            "Camille Durand - Maire de Villeneuve, Vice-Présidente de l'agglomération\n\nChers Habitants,\n\nLe nouveau marché ouvre samedi place de la Mairie.\n\nTexte à la une"
        );
    }

    #[rstest]
    #[case::nbsp("\u{00A0}\u{00A0}")]
    #[case::zero_width("\u{200B}\u{200C}\u{FEFF}")]
    #[case::figure_space("\u{2007}\u{202F}")]
    #[case::only_placeholders("<<logo>> [https://x.mjt.lu/img/a.png]\n\u{00A0}")]
    fn invisible_text_tidies_to_nothing(#[case] text: &str) {
        assert_eq!(tidy(text), "");
    }

    #[rstest]
    fn tidy_leaves_ordinary_text_alone() {
        assert_eq!(
            tidy("Hello Nora,\n\nSee you Monday."),
            "Hello Nora,\n\nSee you Monday."
        );
    }

    /// An event invitation whose text/plain part is the page's HTML source, with
    /// tracking links in the doctype and head.
    const RAW_PLAIN: &str = "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\" \"https://mail.events.example/ls/click?upn=abc123\">\r\n<html dir=\"ltr\" lang=\"en\"><head><meta name=\"viewport\" content=\"width=device-width\"><title>Forum Calcul 2026</title><link rel=\"preload\" as=\"image\" href=\"https://mail.events.example/ls/click?upn=def456\"></head>\r\n<body><div><p>Dear guest,</p><p>Your registration to the Forum Calcul 2026 is confirmed.</p><p>See you on 4 November.</p></div></body></html>";

    #[rstest]
    #[case::doctype(RAW_PLAIN, true)]
    #[case::html_tag("<html><body><p>Bonjour</p></body></html>", true)]
    #[case::prose("Bonjour, voici le <b>plan</b> pour demain.", false)]
    #[case::code_sample("Use </div> to close it.", false)]
    fn raw_html_plain_parts(#[case] text: &str, #[case] want: bool) {
        assert_eq!(looks_like_html(text), want);
    }

    #[rstest]
    fn a_raw_html_plain_part_reads_from_the_html_part() {
        assert_eq!(
            readable(RAW_PLAIN, Some("<p>Your registration is confirmed.</p>")),
            "Your registration is confirmed."
        );
    }

    #[rstest]
    #[case::doctype("<!DOCTYPE")]
    #[case::tag("<p>")]
    #[case::tracking_link("ls/click")]
    #[case::title("</title>")]
    fn raw_html_source_never_shows(#[case] mark: &str) {
        assert!(!clean(&html_text(RAW_PLAIN)).contains(mark));
    }

    #[rstest]
    fn raw_html_source_reads_as_its_words() {
        assert_eq!(
            clean(&html_text(RAW_PLAIN)),
            "Dear guest,\n\nYour registration to the Forum Calcul 2026 is confirmed.\n\nSee you on 4 November."
        );
    }

    #[rstest]
    #[case::break_tag("Merci de ne pas répondre.<BR>", "Merci de ne pas répondre.\n")]
    #[case::paragraphs("<p>Bonjour,</p><p>Voici.</p>", "\nBonjour,\nVoici.\n")]
    #[case::link("<a href=\"https://x.example\">Logout</a>", "Logout")]
    #[case::heart("I <3 it", "I <3 it")]
    #[case::comparison("a < b and c > d", "a < b and c > d")]
    #[case::address("Nora <nora@x.example>", "Nora <nora@x.example>")]
    fn tags_are_removed_from_text(#[case] text: &str, #[case] want: &str) {
        assert_eq!(without_tags(text), want);
    }

    #[rstest]
    fn escaped_markup_in_a_page_never_reads_as_tags() {
        assert_eq!(
            html_text(
                "<p>Menu</p><p>&lt;li&gt;&lt;a href=\"https://x.example/out\"&gt;Logout&lt;/a&gt;&lt;/li&gt;</p>"
            ),
            "\nMenu\n\nLogout\n"
        );
    }

    #[rstest]
    fn the_town_letter_reads_cleanly_as_a_newsletter() {
        assert!(clean(TOWN).starts_with("Camille Durand"));
    }
}
