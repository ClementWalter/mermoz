//! Search by meaning: what each thread's vector is made of, how vectors are stored and
//! compared, and how their ranking fuses with full-text search.
//!
//! Vectors come from an [`Embedder`] (EmbeddingGemma in the daemon, a stub in tests).
//! They are cut to [`DIM`] dimensions, normalised, and kept as int8 with one scale
//! per vector, in their own SQLite file ([`VectorStore`]) and in memory
//! ([`VectorIndex`]) where a query scans every vector: at 100k threads that is 77 MB
//! and a few milliseconds, with no index to maintain.

use crate::error::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use std::path::Path;

/// Dimensions kept per vector: all 768 of EmbeddingGemma's. Its 256-dimension
/// Matryoshka cut lost 7 points of recall@10 and a third of MRR on a real mailbox;
/// int8 lost nothing against float.
pub const DIM: usize = 768;

/// Characters of message text per thread document; EmbeddingGemma reads ~500 tokens.
const DOC_BODY_CHARS: usize = 1500;

/// Reciprocal Rank Fusion constant: rank r adds 1 / (RRF_K + r).
pub const RRF_K: f32 = 60.0;

/// Which caller wants vectors: a search waits on the answer, indexing does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Query,
    Index,
}

/// Turns texts into vectors (any length; they are cut to [`DIM`] and normalised here).
pub trait Embedder: Send + Sync {
    fn embed(&self, texts: &[String], lane: Lane) -> Result<Vec<Vec<f32>>>;
}

/// A stand-in model for tests and demos: each word (beyond the prompt's own) is hashed
/// to one of [`DIM`] dimensions, so texts sharing words are close and others are not.
pub struct StubEmbedder;

/// Words of the prompt forms themselves, which every text carries.
const PROMPT_WORDS: &[&str] = &[
    "task", "search", "result", "query", "title", "text", "from", "none",
];

impl Embedder for StubEmbedder {
    fn embed(&self, texts: &[String], _lane: Lane) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = vec![0f32; 768];
                for w in t
                    .split(|c: char| !c.is_alphanumeric())
                    .map(str::to_lowercase)
                    .filter(|w| w.chars().count() > 2 && !PROMPT_WORDS.contains(&w.as_str()))
                {
                    let h = w
                        .bytes()
                        .fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
                    v[h as usize % DIM] += 1.0;
                }
                v
            })
            .collect())
    }
}

/// One message of a thread, as its document is built from it.
#[derive(Debug, Clone)]
pub struct DocMessage {
    pub sender: String,
    pub is_from_me: bool,
    pub text: String,
}

/// A thread's document, in EmbeddingGemma's retrieval form: the subject as title, then
/// who wrote and the opening messages' text, whitespace collapsed.
pub fn doc_text(subject: &str, messages: &[DocMessage]) -> String {
    let mut who: Vec<&str> = messages
        .iter()
        .map(|m| {
            if m.is_from_me {
                "me"
            } else {
                m.sender.as_str()
            }
        })
        .collect();
    who.sort_unstable();
    who.dedup();
    let body: String = messages
        .iter()
        .take(2)
        .map(|m| m.text.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(DOC_BODY_CHARS)
        .collect();
    let title = if subject.trim().is_empty() {
        "none"
    } else {
        subject.trim()
    };
    format!("title: {title} | text: From {}. {body}", who.join(", "))
}

/// A search in EmbeddingGemma's query form.
pub fn query_text(q: &str) -> String {
    format!("task: search result | query: {}", q.trim())
}

/// A vector cut to [`DIM`], normalised, as int8 with its scale (value = byte / scale).
#[derive(Debug, Clone, PartialEq)]
pub struct Quantized {
    pub scale: f32,
    pub bytes: Vec<i8>,
}

/// Cuts `v` to [`DIM`] dimensions, normalises it and maps its largest component to ±127.
pub fn quantize(v: &[f32]) -> Quantized {
    let v = &v[..v.len().min(DIM)];
    let norm = v
        .iter()
        .map(|x| x * x)
        .sum::<f32>()
        .sqrt()
        .max(f32::MIN_POSITIVE);
    let max = v.iter().fold(0f32, |m, x| m.max(x.abs())) / norm;
    let scale = if max > 0.0 { 127.0 / max } else { 1.0 };
    let bytes = v
        .iter()
        .map(|x| (x / norm * scale).round().clamp(-127.0, 127.0) as i8)
        .collect();
    Quantized { scale, bytes }
}

/// Cosine of two quantized vectors.
pub fn cosine(a: &Quantized, b: &Quantized) -> f32 {
    dot(&a.bytes, &b.bytes) as f32 / (a.scale * b.scale)
}

fn dot(a: &[i8], b: &[i8]) -> i32 {
    // Plain zip-sum: the compiler vectorises it to NEON widening multiply-adds.
    a.iter().zip(b).map(|(x, y)| *x as i32 * *y as i32).sum()
}

/// Every thread's vector, in memory, scanned whole for each query.
#[derive(Debug, Default)]
pub struct VectorIndex {
    ids: Vec<String>,
    sigs: Vec<String>,
    scales: Vec<f32>,
    data: Vec<i8>,
    pos: HashMap<String, usize>,
}

impl VectorIndex {
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// The signature a thread was indexed with, if it was.
    pub fn sig(&self, id: &str) -> Option<&str> {
        self.pos.get(id).map(|&i| self.sigs[i].as_str())
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.ids.iter().map(String::as_str)
    }

    pub fn upsert(&mut self, id: &str, sig: &str, v: &Quantized) {
        let row = &v.bytes[..DIM.min(v.bytes.len())];
        let mut padded = [0i8; DIM];
        padded[..row.len()].copy_from_slice(row);
        match self.pos.get(id) {
            Some(&i) => {
                self.sigs[i] = sig.to_string();
                self.scales[i] = v.scale;
                self.data[i * DIM..(i + 1) * DIM].copy_from_slice(&padded);
            }
            None => {
                self.pos.insert(id.to_string(), self.ids.len());
                self.ids.push(id.to_string());
                self.sigs.push(sig.to_string());
                self.scales.push(v.scale);
                self.data.extend_from_slice(&padded);
            }
        }
    }

    pub fn remove(&mut self, id: &str) {
        let Some(i) = self.pos.remove(id) else { return };
        let last = self.ids.len() - 1;
        if i != last {
            let moved = self.ids[last].clone();
            self.pos.insert(moved, i);
            self.data.copy_within(last * DIM..(last + 1) * DIM, i * DIM);
        }
        self.ids.swap_remove(i);
        self.sigs.swap_remove(i);
        self.scales.swap_remove(i);
        self.data.truncate(last * DIM);
    }

    /// The `k` threads closest to `q` with a cosine of at least `floor`, closest first.
    pub fn nearest(&self, q: &Quantized, k: usize, floor: f32) -> Vec<(String, f32)> {
        let mut scored: Vec<(usize, f32)> = self
            .data
            .chunks_exact(DIM)
            .zip(&self.scales)
            .enumerate()
            .map(|(i, (row, s))| (i, dot(row, &q.bytes[..DIM]) as f32 / (s * q.scale)))
            .filter(|(_, c)| *c >= floor)
            .collect();
        let k = k.min(scored.len());
        if k == 0 {
            return vec![];
        }
        scored.select_nth_unstable_by(k - 1, |a, b| b.1.total_cmp(&a.1));
        scored.truncate(k);
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored
            .into_iter()
            .map(|(i, c)| (self.ids[i].clone(), c))
            .collect()
    }
}

/// Fuses rankings (best first) by Reciprocal Rank Fusion: an item scores the sum of
/// 1 / (RRF_K + rank) over the lists it is in, so agreement beats one strong list.
/// Ties keep the order of first appearance, earlier lists first.
pub fn rrf(lists: &[Vec<String>]) -> Vec<String> {
    let mut score: HashMap<&str, (f32, usize)> = HashMap::new();
    let mut seen = 0usize;
    for list in lists {
        for (r, id) in list.iter().enumerate() {
            let e = score.entry(id.as_str()).or_insert_with(|| {
                seen += 1;
                (0.0, seen)
            });
            e.0 += 1.0 / (RRF_K + 1.0 + r as f32);
        }
    }
    let mut out: Vec<(&str, (f32, usize))> = score.into_iter().collect();
    out.sort_by(|a, b| b.1.0.total_cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
    out.into_iter().map(|(id, _)| id.to_string()).collect()
}

/// The vectors on disk, in their own file next to the mail database: rebuilt from mail
/// if lost, and written without ever taking the mail database's write lock.
pub struct VectorStore {
    conn: Connection,
}

/// Recorded with the vectors; a different value (another model or cut) starts over.
const LAYOUT: &str = "embeddinggemma-300m-q8/768/int8";

impl VectorStore {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS vectors (
               thread_id TEXT PRIMARY KEY, sig TEXT NOT NULL, scale REAL NOT NULL, vec BLOB NOT NULL);",
        )?;
        let layout: Option<String> = conn
            .query_row("SELECT value FROM meta WHERE key = 'layout'", [], |r| {
                r.get(0)
            })
            .optional()?;
        if layout.as_deref() != Some(LAYOUT) {
            conn.execute_batch("DELETE FROM vectors;")?;
            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('layout', ?1)",
                [LAYOUT],
            )?;
        }
        Ok(Self { conn })
    }

    /// Every stored vector, as an in-memory index.
    pub fn load(&self) -> Result<VectorIndex> {
        let mut idx = VectorIndex::default();
        let mut st = self
            .conn
            .prepare("SELECT thread_id, sig, scale, vec FROM vectors")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, f64>(2)?,
                r.get::<_, Vec<u8>>(3)?,
            ))
        })?;
        for row in rows {
            let (id, sig, scale, blob) = row?;
            let q = Quantized {
                scale: scale as f32,
                bytes: blob.into_iter().map(|b| b as i8).collect(),
            };
            idx.upsert(&id, &sig, &q);
        }
        Ok(idx)
    }

    /// Stores `(thread id, signature, vector)` rows in one transaction.
    pub fn put(&mut self, rows: &[(String, String, Quantized)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut st = tx.prepare_cached(
                "INSERT OR REPLACE INTO vectors (thread_id, sig, scale, vec) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (id, sig, q) in rows {
                let blob: Vec<u8> = q.bytes.iter().map(|b| *b as u8).collect();
                st.execute(params![id, sig, q.scale as f64, blob])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete(&mut self, ids: &[String]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut st = tx.prepare_cached("DELETE FROM vectors WHERE thread_id = ?1")?;
            for id in ids {
                st.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::{fixture, rstest};

    fn msg(sender: &str, me: bool, text: &str) -> DocMessage {
        DocMessage {
            sender: sender.into(),
            is_from_me: me,
            text: text.into(),
        }
    }

    fn unit(i: usize) -> Vec<f32> {
        let mut v = vec![0.0; 768];
        v[i] = 1.0;
        v
    }

    #[rstest]
    fn doc_text_puts_the_subject_as_title() {
        assert!(doc_text("Lunch", &[msg("Ana", false, "hi")]).starts_with("title: Lunch | text: "));
    }

    #[rstest]
    fn doc_text_without_subject_says_none() {
        assert!(doc_text(" ", &[msg("Ana", false, "hi")]).starts_with("title: none |"));
    }

    #[rstest]
    fn doc_text_lists_each_writer_once_with_me() {
        let d = doc_text(
            "S",
            &[
                msg("Ana", false, "a"),
                msg("x", true, "b"),
                msg("Ana", false, "c"),
            ],
        );
        assert!(d.contains("From Ana, me. "));
    }

    #[rstest]
    fn doc_text_reads_the_first_two_messages() {
        let d = doc_text(
            "S",
            &[
                msg("A", false, "one"),
                msg("B", false, "two"),
                msg("C", false, "three"),
            ],
        );
        assert!(d.ends_with(". one two"));
    }

    #[rstest]
    fn doc_text_collapses_whitespace() {
        assert!(doc_text("S", &[msg("A", false, "a\n\n  b")]).ends_with(". a b"));
    }

    #[rstest]
    fn doc_text_caps_the_body() {
        let d = doc_text("S", &[msg("A", false, &"q".repeat(5000))]);
        assert_eq!(d.matches('q').count(), DOC_BODY_CHARS);
    }

    #[rstest]
    fn query_text_has_the_search_prefix() {
        assert_eq!(query_text(" lunch "), "task: search result | query: lunch");
    }

    #[rstest]
    fn the_stub_puts_shared_words_close() {
        let v = StubEmbedder
            .embed(
                &[
                    query_text("ferry"),
                    "title: Ferry | text: From Ana. noon".into(),
                    "title: Bus | text: From Ana. noon".into(),
                ],
                Lane::Index,
            )
            .unwrap();
        let q: Vec<Quantized> = v.iter().map(|x| quantize(x)).collect();
        assert!(cosine(&q[0], &q[1]) > cosine(&q[0], &q[2]));
    }

    #[rstest]
    fn quantize_cuts_to_dim() {
        assert_eq!(quantize(&unit(0)).bytes.len(), DIM);
    }

    #[rstest]
    fn quantize_maps_the_largest_component_to_127() {
        assert_eq!(quantize(&[3.0, -4.0]).bytes, vec![95, -127]);
    }

    #[rstest]
    fn cosine_of_a_vector_with_itself_is_one() {
        let q = quantize(&[0.3, 0.5, -0.2, 0.9]);
        assert!((cosine(&q, &q) - 1.0).abs() < 0.01);
    }

    #[rstest]
    fn cosine_of_orthogonal_vectors_is_zero() {
        assert_eq!(cosine(&quantize(&unit(1)), &quantize(&unit(2))), 0.0);
    }

    #[fixture]
    fn index() -> VectorIndex {
        let mut idx = VectorIndex::default();
        idx.upsert("a", "1", &quantize(&unit(0)));
        idx.upsert("b", "1", &quantize(&unit(1)));
        let mut c = unit(0);
        c[1] = 1.0;
        idx.upsert("c", "1", &quantize(&c));
        idx
    }

    #[rstest]
    fn nearest_ranks_the_closest_first(index: VectorIndex) {
        let got: Vec<String> = index
            .nearest(&quantize(&unit(0)), 3, -1.0)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(got, vec!["a", "c", "b"]);
    }

    #[rstest]
    fn nearest_drops_vectors_below_the_floor(index: VectorIndex) {
        assert_eq!(index.nearest(&quantize(&unit(0)), 3, 0.5).len(), 2);
    }

    #[rstest]
    fn nearest_returns_at_most_k(index: VectorIndex) {
        assert_eq!(index.nearest(&quantize(&unit(0)), 1, -1.0).len(), 1);
    }

    #[rstest]
    fn upsert_replaces_a_thread_vector(mut index: VectorIndex) {
        index.upsert("a", "2", &quantize(&unit(1)));
        assert_eq!(index.nearest(&quantize(&unit(0)), 1, -1.0)[0].0, "c");
    }

    #[rstest]
    fn upsert_records_the_signature(mut index: VectorIndex) {
        index.upsert("a", "2", &quantize(&unit(1)));
        assert_eq!(index.sig("a"), Some("2"));
    }

    #[rstest]
    fn remove_forgets_the_thread(mut index: VectorIndex) {
        index.remove("a");
        assert_eq!(index.sig("a"), None);
    }

    #[rstest]
    fn remove_keeps_the_moved_vector_findable(mut index: VectorIndex) {
        index.remove("a");
        let mut c = unit(0);
        c[1] = 1.0;
        assert_eq!(index.nearest(&quantize(&c), 1, -1.0)[0].0, "c");
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[rstest]
    fn rrf_puts_agreement_first() {
        assert_eq!(
            rrf(&[ids(&["a", "b"]), ids(&["b", "c"])])[0],
            "b".to_string()
        );
    }

    #[rstest]
    fn rrf_breaks_ties_by_first_list() {
        assert_eq!(rrf(&[ids(&["a"]), ids(&["c"])]), ids(&["a", "c"]));
    }

    #[rstest]
    fn rrf_keeps_every_item_once() {
        assert_eq!(rrf(&[ids(&["a", "b"]), ids(&["b", "a", "c"])]).len(), 3);
    }

    #[fixture]
    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[rstest]
    fn store_keeps_vectors_across_reopen(dir: tempfile::TempDir) {
        let path = dir.path().join("v.sqlite");
        VectorStore::open(&path)
            .unwrap()
            .put(&[("t1".into(), "s".into(), quantize(&unit(3)))])
            .unwrap();
        let idx = VectorStore::open(&path).unwrap().load().unwrap();
        assert_eq!(idx.nearest(&quantize(&unit(3)), 1, 0.9)[0].0, "t1");
    }

    #[rstest]
    fn store_delete_removes_the_vector(dir: tempfile::TempDir) {
        let path = dir.path().join("v.sqlite");
        let mut s = VectorStore::open(&path).unwrap();
        s.put(&[("t1".into(), "s".into(), quantize(&unit(3)))])
            .unwrap();
        s.delete(&["t1".into()]).unwrap();
        assert!(s.load().unwrap().is_empty());
    }

    #[rstest]
    fn store_starts_over_for_another_layout(dir: tempfile::TempDir) {
        let path = dir.path().join("v.sqlite");
        let s = VectorStore::open(&path).unwrap();
        s.conn
            .execute("UPDATE meta SET value = 'old' WHERE key = 'layout'", [])
            .unwrap();
        let mut s = s;
        s.put(&[("t1".into(), "s".into(), quantize(&unit(3)))])
            .unwrap();
        drop(s);
        assert!(VectorStore::open(&path).unwrap().load().unwrap().is_empty());
    }
}
