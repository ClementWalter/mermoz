//! Which message a reaction ("[heart] Nora reacted to your message") is to: the one
//! its References / In-Reply-To name, else the earlier message that opens on the words
//! the reaction quotes.

use crate::model::Message;

fn bare(id: &str) -> &str {
    id.trim().trim_matches(['<', '>'])
}

/// Words of a text on one line, lowercased, for comparing openings.
fn flat(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Sets each reaction's `target_message_id`. `ids` holds every message's (Message-ID,
/// References) and `quotes` each reaction's quoted opening, in the order of `msgs`.
pub(super) fn find_targets(
    msgs: &mut [Message],
    ids: &[(String, Vec<String>)],
    quotes: &[Option<String>],
) {
    for i in 0..msgs.len() {
        if msgs[i].reaction.is_none() {
            continue;
        }
        // In-Reply-To is the last reference.
        let by_header = ids[i].1.last().and_then(|parent| {
            (0..msgs.len())
                .find(|&k| k != i && !ids[k].0.is_empty() && bare(&ids[k].0) == bare(parent))
        });
        let by_quote = || {
            let q = flat(quotes[i].as_deref()?);
            let probe: String = q.chars().take(60).collect();
            if probe.chars().count() < 8 {
                return None;
            }
            (0..i).rev().find(|&k| {
                msgs[k].reaction.is_none() && flat(&msgs[k].body_new).starts_with(&probe)
            })
        };
        let target = by_header.or_else(by_quote).map(|k| msgs[k].id.clone());
        if let Some(r) = msgs[i].reaction.as_mut() {
            r.target_message_id = target;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::mime::ParsedMessage;
    use crate::model::Address;
    use crate::store::{IncomingMessage, Store, SyncMode};
    use rstest::*;

    struct Env {
        _dir: tempfile::TempDir,
        store: Store,
        thread: String,
    }

    const MINE: &str =
        "Bonjour Madame\n\nNous ne serons pas là jeudi, nous partons en vacances.\n\nCordialement";

    fn mail(
        pid: &str,
        date: i64,
        from: &str,
        text: &str,
        mid: &str,
        refs: &[&str],
    ) -> IncomingMessage {
        IncomingMessage {
            provider_id: pid.into(),
            provider_thread_id: "t1".into(),
            label_ids: vec!["INBOX".into()],
            internal_date: date,
            parsed: ParsedMessage {
                from: Some(Address {
                    name: from.into(),
                    email: format!("{}@mail.example", from.to_lowercase().replace(' ', ".")),
                }),
                subject: "Re: Vacances".into(),
                body_text: text.into(),
                message_id: Some(mid.into()),
                references: refs.iter().map(|r| r.to_string()).collect(),
                ..Default::default()
            },
        }
    }

    fn reaction_text(lang: &str) -> String {
        let line = match lang {
            "fr" => "[heart]   CRÈCHE DU PARC a réagi à votre message :",
            _ => "[heart]   CRECHE DU PARC reacted to your message:",
        };
        format!(
            "{line}\n________________________________\nFrom: Teo Blanc <teo@mail.example>\nSent: Wednesday, 29 July 2026 12:10\nTo: Crèche du Parc <parc@mail.example>\nSubject: Re: Vacances\n\n{MINE}\n"
        )
    }

    fn env_with(lang: &str, refs: &[&str]) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
        let acc = store.add_account("teo@mail.example", "gmail").unwrap().id;
        let thread = store
            .ingest_many(
                &acc,
                "teo@mail.example",
                &[
                    mail(
                        "m1",
                        1_700_000_000_000,
                        "Teo",
                        MINE,
                        "mine-1@mail.example",
                        &[],
                    ),
                    mail(
                        "m2",
                        1_700_000_100_000,
                        "Creche",
                        &reaction_text(lang),
                        "react-1@mail.example",
                        refs,
                    ),
                ],
                SyncMode::Initial,
            )
            .unwrap()
            .remove(0);
        Env {
            _dir: dir,
            store,
            thread,
        }
    }

    fn reaction(env: &Env) -> crate::model::Reaction {
        env.store.messages(&env.thread).unwrap()[1]
            .reaction
            .clone()
            .unwrap()
    }

    fn first_id(env: &Env) -> String {
        env.store.messages(&env.thread).unwrap()[0].id.clone()
    }

    #[rstest]
    #[case::english("en")]
    #[case::french("fr")]
    fn a_reaction_reads_as_its_emoji(#[case] lang: &str) {
        assert_eq!(
            reaction(&env_with(lang, &["mine-1@mail.example"])).emoji,
            "❤️"
        );
    }

    #[rstest]
    fn the_target_comes_from_in_reply_to() {
        let env = env_with("en", &["older@mail.example", "<mine-1@mail.example>"]);
        assert_eq!(reaction(&env).target_message_id, Some(first_id(&env)));
    }

    #[rstest]
    fn without_references_the_quoted_words_find_the_target() {
        let env = env_with("fr", &[]);
        assert_eq!(reaction(&env).target_message_id, Some(first_id(&env)));
    }

    #[rstest]
    fn a_reaction_bubble_text_is_the_emoji() {
        let env = env_with("en", &[]);
        assert_eq!(env.store.messages(&env.thread).unwrap()[1].body_new, "❤️");
    }
}
