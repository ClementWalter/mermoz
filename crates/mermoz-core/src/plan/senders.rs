//! Who a plan's "from" means. A sender is its display names, its address and its
//! mailing list, all cut into words: "esrin gourmet club" finds
//! `esrin-gourmet-club@agency.example` even with an empty display name, through
//! dots, hyphens, underscores, plus signs, accents and a small typo.

/// One sender in the mailbox: an address with every name it wrote under.
#[derive(Debug, Clone, PartialEq)]
pub struct Sender {
    pub email: String,
    /// Display names seen, space separated (may be empty).
    pub names: String,
    /// The mailing list (List-Id) its mail came through, if any.
    pub list: Option<String>,
    pub threads: i64,
}

/// What one "from" entry asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// "nora@lind.example": that address only.
    Address(String),
    /// "news.example": its addresses and its subdomains'.
    Domain(String),
    /// Anything else: words to find in names, address and list.
    Words(Vec<String>),
}

/// Words that say nothing about who wrote ("all the mail from the …").
const FILLER: &[&str] = &[
    "a", "an", "the", "of", "from", "by", "to", "and", "all", "every", "any", "sent", "mail",
    "mails", "email", "emails", "message", "messages", "de", "du", "des", "le", "la", "les", "l",
    "d", "un", "une", "et", "par", "tous", "toutes", "tout",
];

/// Lowercase without accents: "Café Élysée" → "cafe elysee".
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => out.push('a'),
            'ç' | 'ć' | 'č' => out.push('c'),
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => out.push('e'),
            'ì' | 'í' | 'î' | 'ï' | 'ī' => out.push('i'),
            'ñ' | 'ń' | 'ň' => out.push('n'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => out.push('o'),
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' => out.push('u'),
            'ý' | 'ÿ' => out.push('y'),
            'š' | 'ś' => out.push('s'),
            'ž' | 'ź' | 'ż' => out.push('z'),
            'ł' => out.push('l'),
            'ř' => out.push('r'),
            'œ' => out.push_str("oe"),
            'æ' => out.push_str("ae"),
            'ß' => out.push_str("ss"),
            c => out.push(c),
        }
    }
    out
}

/// Folded words, cut on anything that is not a letter or digit (`.`, `-`, `_`, `+`, `@`…).
pub fn words(s: &str) -> Vec<String> {
    fold(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Words of an instruction that say what it is about, not what to do or when.
const NOT_CONTENT: &[&str] = &[
    "delete",
    "trash",
    "remove",
    "archive",
    "archiver",
    "mark",
    "move",
    "put",
    "file",
    "label",
    "tag",
    "add",
    "set",
    "clean",
    "clear",
    "sort",
    "snooze",
    "later",
    "read",
    "unread",
    "done",
    "like",
    "similar",
    "older",
    "newer",
    "than",
    "past",
    "last",
    "ago",
    "before",
    "after",
    "day",
    "days",
    "week",
    "weeks",
    "month",
    "months",
    "year",
    "years",
    "not",
    "no",
    "in",
    "into",
    "on",
    "with",
    "without",
    "as",
    "is",
    "are",
    "that",
    "which",
    "these",
    "those",
    "this",
    "my",
    "me",
    "i",
    "please",
    "thread",
    "threads",
    "supprime",
    "supprimer",
    "efface",
    "marque",
    "deplace",
    "range",
    "classe",
    "mets",
    "comme",
    "plus",
    "vieux",
    "anciens",
    "mois",
    "jours",
    "ans",
    "dans",
    "pas",
    "mes",
    "ces",
];

/// The instruction's content words ("delete all the messages like notes from gemini"
/// → notes, gemini), for finding candidates in the mailbox.
pub fn content_words(sentence: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for w in words(sentence) {
        let skip = FILLER.contains(&w.as_str())
            || NOT_CONTENT.contains(&w.as_str())
            || w.chars().all(|c| c.is_ascii_digit())
            || w.chars().count() < 2;
        if !skip && !out.contains(&w) {
            out.push(w);
        }
    }
    out
}

/// Reads one "from" entry.
pub fn entry(raw: &str) -> Entry {
    let s = fold(raw.trim());
    if s.contains('@') && !s.contains(' ') {
        return Entry::Address(s);
    }
    if s.contains('.') && !s.contains(' ') {
        return Entry::Domain(s.trim_start_matches('@').to_string());
    }
    let all = words(&s);
    let content: Vec<String> = all
        .iter()
        .filter(|w| !FILLER.contains(&w.as_str()))
        .cloned()
        .collect();
    Entry::Words(if content.is_empty() { all } else { content })
}

/// Edit distance, for typos: a swap of two letters counts as one edit.
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        d[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = v;
        }
    }
    d[a.len()][b.len()]
}

/// A sender's words, and the same run together (for "gourmetclub@…").
fn sender_words(s: &Sender) -> (Vec<String>, String) {
    let mut w = words(&s.names);
    w.extend(words(&s.email));
    if let Some(l) = &s.list {
        w.extend(words(l));
    }
    let joined = w.concat();
    (w, joined)
}

/// One typed word found among a sender's words: the same word, the start of one, inside
/// words run together, or one letter off (two for long words).
fn found(t: &str, words: &[String], joined: &str) -> bool {
    let n = t.chars().count();
    words.iter().any(|w| {
        w == t
            || (n >= 3 && w.starts_with(t))
            || (n >= 5 && distance(t, w) <= 1)
            || (n >= 8 && distance(t, w) <= 2)
    }) || (n >= 4 && joined.contains(t))
}

/// How many of the typed words the sender has.
pub fn hits(typed: &[String], s: &Sender) -> usize {
    let (w, joined) = sender_words(s);
    typed.iter().filter(|t| found(t, &w, &joined)).count()
}

/// The entry names this sender: the address, the domain, or every word.
pub fn matches(e: &Entry, s: &Sender) -> bool {
    let email = fold(&s.email);
    match e {
        Entry::Address(a) => email == *a,
        Entry::Domain(d) => email.ends_with(&format!("@{d}")) || email.ends_with(&format!(".{d}")),
        Entry::Words(t) => !t.is_empty() && hits(t, s) == t.len(),
    }
}

/// Senders closest to entries that matched no one: most words found first, then the
/// most threads. Senders sharing no word are left out.
pub fn closest<'a>(entries: &[Entry], senders: &'a [Sender], limit: usize) -> Vec<&'a Sender> {
    let typed: Vec<String> = entries
        .iter()
        .flat_map(|e| match e {
            Entry::Words(t) => t.clone(),
            Entry::Address(a) => words(a),
            Entry::Domain(d) => words(d),
        })
        .collect();
    let mut scored: Vec<(usize, &Sender)> = senders
        .iter()
        .map(|s| (hits(&typed, s), s))
        .filter(|(h, _)| *h > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.threads.cmp(&a.1.threads)));
    scored.into_iter().take(limit).map(|(_, s)| s).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn sender(email: &str, names: &str) -> Sender {
        Sender {
            email: email.into(),
            names: names.into(),
            list: None,
            threads: 1,
        }
    }

    #[rstest]
    #[case::hyphens("esrin-gourmet-club@agency.example")]
    #[case::dots("esrin.gourmet.club@agency.example")]
    #[case::underscores("esrin_gourmet_club@agency.example")]
    #[case::plus("esrin+gourmet-club@agency.example")]
    #[case::run_together("esringourmetclub@agency.example")]
    #[case::mixed_case("ESRIN-Gourmet.Club@Agency.example")]
    fn words_in_an_address_with_no_name_match(#[case] email: &str) {
        assert!(matches(&entry("esrin gourmet club"), &sender(email, "")));
    }

    #[rstest]
    fn filler_words_are_ignored() {
        assert!(matches(
            &entry("all the mail from the esrin gourmet club"),
            &sender("esrin-gourmet-club@agency.example", "")
        ));
    }

    #[rstest]
    #[case::accent_typed("café des arts", "cafe.des.arts@town.example", "")]
    #[case::accent_in_name("cafe des arts", "info@town.example", "Café des Arts")]
    fn accents_are_folded(#[case] typed: &str, #[case] email: &str, #[case] names: &str) {
        assert!(matches(&entry(typed), &sender(email, names)));
    }

    #[rstest]
    #[case::missing_letter("esrin gourmt club")]
    #[case::swapped_letters("esrin gorumet club")]
    fn a_small_typo_still_matches(#[case] typed: &str) {
        assert!(matches(
            &entry(typed),
            &sender("esrin-gourmet-club@agency.example", "")
        ));
    }

    #[rstest]
    fn every_word_must_be_found() {
        assert!(!matches(
            &entry("esrin chess club"),
            &sender("esrin-gourmet-club@agency.example", "")
        ));
    }

    #[rstest]
    fn a_name_matches_by_its_words() {
        assert!(matches(
            &entry("nora"),
            &sender("n.lind@lind.example", "Nora Lind")
        ));
    }

    #[rstest]
    fn a_list_name_counts() {
        let s = Sender {
            list: Some("gourmet-club.lists.agency.example".into()),
            ..sender("noreply@agency.example", "")
        };
        assert!(matches(&entry("gourmet club"), &s));
    }

    #[rstest]
    fn an_address_matches_only_itself() {
        assert!(!matches(
            &entry("nora@lind.example"),
            &sender("nora.b@lind.example", "Nora")
        ));
    }

    #[rstest]
    #[case::exact("news.example", "digest@news.example", true)]
    #[case::subdomain("news.example", "a@mail.news.example", true)]
    #[case::other("news.example", "news@example.org", false)]
    fn a_domain_matches_its_addresses(
        #[case] typed: &str,
        #[case] email: &str,
        #[case] want: bool,
    ) {
        assert_eq!(matches(&entry(typed), &sender(email, "")), want);
    }

    #[rstest]
    fn closest_ranks_by_words_found_then_threads() {
        let senders = vec![
            Sender {
                threads: 50,
                ..sender("club@golf.example", "")
            },
            Sender {
                threads: 34,
                ..sender("gourmet-club@agency.example", "")
            },
            Sender {
                threads: 9,
                ..sender("paul@roux.example", "Paul")
            },
        ];
        let got: Vec<&str> = closest(&[entry("esrin gourmet club")], &senders, 3)
            .iter()
            .map(|s| s.email.as_str())
            .collect();
        assert_eq!(
            got,
            vec!["gourmet-club@agency.example", "club@golf.example"]
        );
    }
}
