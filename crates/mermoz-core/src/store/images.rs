//! Which stored images are decoration (see [`crate::decorative`]). New mail is
//! classified at ingest from the image bytes; older mail when the background reading
//! pass reaches it (its first bytes read from the blob file); until then a thread
//! classifies by name, type and size. Images repeated across a conversation's messages
//! from one sender are decoration whatever was stored.

use crate::decorative::{self, Image};
use crate::error::Result;
use crate::mime::ParsedAttachment;
use crate::model::Message;
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::io::Read;

/// `decorative` is null until the image was looked at; readings learn of reactions.
pub(super) const MIGRATION: &str = "ALTER TABLE attachments ADD COLUMN decorative INTEGER;
ALTER TABLE message_reading ADD COLUMN reaction TEXT;
ALTER TABLE message_reading ADD COLUMN reaction_quote TEXT;";

/// Bytes read from an image file: enough for any header's width and height.
const HEAD: usize = 64 * 1024;

/// The byte where a message's own words end in its HTML: its first quote marker.
pub(super) fn content_end(html: &str) -> usize {
    let (cut, quoted) = crate::quote::split_html(html);
    if quoted { cut.len() } else { html.len() }
}

/// In how many other messages (up to 2, all the rule needs) this sender sent an image
/// of this size and type.
fn sent_elsewhere(
    conn: &Connection,
    from: &str,
    size: i64,
    mime: &str,
    except: &str,
) -> Result<usize> {
    Ok(conn
        .prepare_cached(
            "SELECT COUNT(*) FROM (SELECT DISTINCT a.message_id FROM messages m
               JOIN attachments a ON a.message_id = m.id
             WHERE lower(m.from_email) = lower(?1) AND a.size = ?2 AND a.mime = ?3
               AND a.message_id != ?4 LIMIT 2)",
        )?
        .query_row(params![from, size, mime, except], |r| r.get::<_, i64>(0))? as usize)
}

/// The decorative flag of each attachment of a message being stored.
pub(super) fn classify_incoming(
    conn: &Connection,
    from: &str,
    message_id: &str,
    attachments: &[ParsedAttachment],
    html: Option<&str>,
) -> Result<Vec<bool>> {
    let end = html.map_or(0, content_end);
    let mut out = Vec::with_capacity(attachments.len());
    for a in attachments {
        let size = a.data.len() as i64;
        let here = attachments
            .iter()
            .filter(|b| b.data.len() as i64 == size && b.mime == a.mime)
            .count()
            - 1;
        let img = Image {
            filename: &a.filename,
            mime: &a.mime,
            size,
            inline: a.inline,
            content_id: a.content_id.as_deref(),
            head: &a.data[..a.data.len().min(HEAD)],
        };
        // The database is asked only when nothing cheaper decided.
        let decorative = decorative::is_decorative(&img, html, end, here)
            || img.mime.starts_with("image/")
                && decorative::is_decorative(
                    &img,
                    None,
                    0,
                    here + sent_elsewhere(conn, from, size, &a.mime, message_id)?,
                );
        out.push(decorative);
    }
    Ok(out)
}

/// An attachment of a stored message not classified yet.
pub(super) struct Unclassified {
    pub id: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    pub inline: bool,
    pub content_id: Option<String>,
    pub path: String,
}

/// A stored message's unclassified attachments.
pub(super) fn unclassified(conn: &Connection, message_id: &str) -> Result<Vec<Unclassified>> {
    Ok(conn
        .prepare_cached(
            "SELECT id, filename, mime, size, inline, content_id, path FROM attachments
             WHERE message_id = ?1 AND decorative IS NULL",
        )?
        .query_map([message_id], |r| {
            Ok(Unclassified {
                id: r.get(0)?,
                filename: r.get(1)?,
                mime: r.get(2)?,
                size: r.get(3)?,
                inline: r.get::<_, i64>(4)? != 0,
                content_id: r.get(5)?,
                path: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

/// Decorative flags for a stored message's attachments, reading each image's first
/// bytes from its file (outside any write lock).
pub(super) fn classify_stored(
    conn: &Connection,
    from: &str,
    message_id: &str,
    html: Option<&str>,
    list: &[Unclassified],
) -> Result<Vec<(String, bool)>> {
    let end = html.map_or(0, content_end);
    let mut out = vec![];
    for a in list {
        let mut head = vec![];
        if a.mime.starts_with("image/")
            && let Ok(f) = std::fs::File::open(&a.path)
        {
            let _ = f.take(HEAD as u64).read_to_end(&mut head);
        }
        let here = list
            .iter()
            .filter(|b| b.size == a.size && b.mime == a.mime)
            .count()
            - 1;
        let img = Image {
            filename: &a.filename,
            mime: &a.mime,
            size: a.size,
            inline: a.inline,
            content_id: a.content_id.as_deref(),
            head: &head,
        };
        let decorative = decorative::is_decorative(&img, html, end, here)
            || a.mime.starts_with("image/")
                && decorative::is_decorative(
                    &img,
                    None,
                    0,
                    here + sent_elsewhere(conn, from, a.size, &a.mime, message_id)?,
                );
        out.push((a.id.clone(), decorative));
    }
    Ok(out)
}

/// Images one sender sent in several messages of the conversation (a signature banner,
/// a logo) are decoration, by the rule of [`decorative::is_decorative`].
pub(super) fn mark_repeated(msgs: &mut [Message]) {
    let mut seen: HashMap<(String, i64, String), std::collections::HashSet<usize>> = HashMap::new();
    for (i, m) in msgs.iter().enumerate() {
        for a in m
            .attachments
            .iter()
            .filter(|a| a.mime.starts_with("image/"))
        {
            seen.entry((m.from.email.to_lowercase(), a.size, a.mime.clone()))
                .or_default()
                .insert(i);
        }
    }
    for m in msgs.iter_mut() {
        let from = m.from.email.to_lowercase();
        for a in m.attachments.iter_mut() {
            let messages = seen
                .get(&(from.clone(), a.size, a.mime.clone()))
                .map_or(0, |s| s.len());
            let img = Image {
                filename: &a.filename,
                mime: &a.mime,
                size: a.size,
                inline: a.inline,
                ..Default::default()
            };
            if messages > 1 && decorative::is_decorative(&img, None, 0, messages - 1) {
                a.decorative = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::mime::{ParsedAttachment, ParsedMessage};
    use crate::model::Address;
    use crate::store::{IncomingMessage, Store, SyncMode};
    use rstest::*;

    struct Env {
        _dir: tempfile::TempDir,
        store: Store,
        thread: String,
    }

    /// A fictional PNG header of the given size, padded to `bytes`.
    fn png(w: u32, h: u32, bytes: usize) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.resize(bytes, 0);
        b
    }

    fn image(name: &str, data: Vec<u8>, cid: &str) -> ParsedAttachment {
        ParsedAttachment {
            filename: name.into(),
            mime: "image/png".into(),
            data,
            content_id: Some(cid.into()),
            inline: true,
        }
    }

    /// A nursery's message: a pasted photo of the day, then its signature (a banner
    /// and two social icons), the banner repeated by every message.
    fn message(pid: &str, date: i64, photo: bool) -> IncomingMessage {
        let mut attachments = vec![
            image(
                "image.png",
                png(600, 300, 110_000),
                &format!("banner-{pid}"),
            ),
            image("image.png", png(24, 24, 1_200), &format!("fb-{pid}")),
            image("linkedin.png", png(24, 24, 1_300), &format!("in-{pid}")),
        ];
        if photo {
            attachments.push(image(
                "image001.png",
                png(1200, 900, 300_000),
                &format!("photo-{pid}"),
            ));
        }
        IncomingMessage {
            provider_id: pid.into(),
            provider_thread_id: "t1".into(),
            label_ids: vec!["INBOX".into()],
            internal_date: date,
            parsed: ParsedMessage {
                from: Some(Address {
                    name: "Crèche du Parc".into(),
                    email: "parc@creche.example".into(),
                }),
                subject: "Journée de Léo".into(),
                body_text: "Bonne journée aujourd'hui.".into(),
                body_html: Some(format!(
                    r#"<p>Bonne journée aujourd'hui.</p><img src="cid:photo-{pid}"><p>Crèche du Parc</p><img src="cid:banner-{pid}"><img src="cid:fb-{pid}">"#
                )),
                attachments,
                ..Default::default()
            },
        }
    }

    #[fixture]
    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("db.sqlite"), &dir.path().join("blobs")).unwrap();
        let acc = store.add_account("me@example.com", "gmail").unwrap().id;
        let thread = store
            .ingest_many(
                &acc,
                "me@example.com",
                &[
                    message("m1", 1_700_000_000_000, true),
                    message("m2", 1_700_000_100_000, false),
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

    fn content_names(env: &Env, k: usize) -> Vec<String> {
        env.store.messages(&env.thread).unwrap()[k]
            .attachments
            .iter()
            .filter(|a| !a.decorative)
            .map(|a| a.filename.clone())
            .collect()
    }

    #[rstest]
    fn only_the_pasted_photo_is_content(env: Env) {
        assert_eq!(content_names(&env, 0), vec!["image001.png"]);
    }

    #[rstest]
    fn a_message_with_only_its_signature_has_no_content_image(env: Env) {
        assert!(content_names(&env, 1).is_empty());
    }

    #[rstest]
    fn the_photos_tab_counts_content_images_only(env: Env) {
        let msgs = env.store.messages(&env.thread).unwrap();
        let photos = env.store.conversation(&env.thread, &msgs).unwrap().files;
        assert_eq!(photos.len(), 1);
    }

    #[rstest]
    fn the_background_pass_classifies_older_mail(env: Env) {
        env.store
            .w
            .lock()
            .unwrap()
            .execute_batch("UPDATE attachments SET decorative = NULL; DELETE FROM message_reading; DELETE FROM meta WHERE key = 'reading_version'")
            .unwrap();
        while env.store.read_bodies_batch(0, 10).unwrap().is_some() {}
        let unclassified: i64 = env
            .store
            .w
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM attachments WHERE decorative IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(unclassified, 0);
    }
}
