//! Known-item search quality on a real mailbox, run by hand, never in CI:
//!
//! ```text
//! MERMOZ_EMBED_MODEL=…/embeddinggemma-300M-Q8_0.gguf \
//! MERMOZ_EVAL_DB=/tmp/copy.sqlite MERMOZ_EVAL_QUERIES=/tmp/queries.json \
//! MERMOZ_EVAL_CACHE=/tmp/vectors.bin \
//! cargo test --release -p mermozd semantic_eval -- --ignored --nocapture
//! ```
//!
//! The database must be a copy (the store migrates it). Queries are `[{"thread": id,
//! "query": text}]`. Reports recall@1/5/10 and MRR for full-text search, vectors (768d
//! float, 768d and 256d int8) and their fusion, with indexing and query timings.

use super::llama::LlamaEmbedder;
use mermoz_core::semantic::{
    DIM, Embedder, Lane, Quantized, VectorIndex, cosine, quantize, query_text, rrf,
};
use mermoz_core::store::Store;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;
use tracing::info;

const CORPUS: usize = 10_000;

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} is required"))
}

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .unwrap_or(0.0)
        / 1024.0
}

fn unit(v: &[f32], d: usize) -> Vec<f32> {
    let v = &v[..d];
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    v.iter().map(|x| x / n).collect()
}

/// 1-based rank of the first of `want` in `list`.
fn rank(list: &[String], want: &[String]) -> Option<usize> {
    list.iter().position(|t| want.contains(t)).map(|i| i + 1)
}

/// The first `d` dimensions, normalised, as int8.
fn quant_cut(v: &[f32], d: usize) -> Quantized {
    let u = unit(v, d);
    let max = u.iter().fold(0f32, |m, x| m.max(x.abs()));
    let scale = 127.0 / max;
    Quantized {
        scale,
        bytes: u.iter().map(|x| (x * scale).round() as i8).collect(),
    }
}

fn report(name: &str, ranks: &[Option<usize>]) {
    let n = ranks.len() as f64;
    let at =
        |k: usize| 100.0 * ranks.iter().filter(|r| r.is_some_and(|r| r <= k)).count() as f64 / n;
    let mrr = ranks.iter().flatten().map(|r| 1.0 / *r as f64).sum::<f64>() / n;
    info!(
        "{name:<22} R@1 {:>3.0}%  R@5 {:>3.0}%  R@10 {:>3.0}%  MRR {mrr:.2}",
        at(1),
        at(5),
        at(10)
    );
}

fn load_or_embed(model: &LlamaEmbedder, docs: &[(String, String)]) -> Vec<Vec<f32>> {
    let cache = std::env::var("MERMOZ_EVAL_CACHE").ok().map(PathBuf::from);
    let mut out: Vec<Vec<f32>> = vec![];
    if let Some(c) = cache.as_ref().filter(|c| c.exists()) {
        let bytes = std::fs::read(c).unwrap();
        let floats: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        info!("vectors from {}", c.display());
        out = floats.chunks_exact(768).map(<[f32]>::to_vec).collect();
        out.truncate(docs.len());
    }
    let t = Instant::now();
    let start = out.len();
    for (i, chunk) in docs[start..].chunks(32).enumerate() {
        let texts: Vec<String> = chunk.iter().map(|(_, d)| d.clone()).collect();
        out.extend(model.embed(&texts, Lane::Index).unwrap());
        if i % 50 == 0 {
            info!(
                "embedded {}/{} ({:.1} threads/s, RSS {:.0} MB)",
                out.len(),
                docs.len(),
                out.len() as f64 / t.elapsed().as_secs_f64(),
                rss_mb()
            );
        }
    }
    info!(
        "indexed {} threads in {:.0} s ({:.1} threads/s), RSS {:.0} MB",
        out.len(),
        t.elapsed().as_secs_f64(),
        out.len() as f64 / t.elapsed().as_secs_f64(),
        rss_mb()
    );
    if let Some(c) = cache {
        let bytes: Vec<u8> = out.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
        std::fs::write(c, bytes).unwrap();
    }
    out
}

#[test]
#[ignore = "needs the real model and a copy of a real mailbox"]
fn semantic_eval() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let store = Store::open(&PathBuf::from(env("MERMOZ_EVAL_DB")), &std::env::temp_dir()).unwrap();
    let queries: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(env("MERMOZ_EVAL_QUERIES")).unwrap())
            .unwrap();
    let t = Instant::now();
    let model = LlamaEmbedder::load(&PathBuf::from(env("MERMOZ_EMBED_MODEL"))).unwrap();
    info!(
        "model loaded in {:.1} s, RSS {:.0} MB",
        t.elapsed().as_secs_f64(),
        rss_mb()
    );

    // The newest threads, then any target older than them (known-item search).
    let mut corpus: Vec<String> = store
        .semantic_sigs()
        .unwrap()
        .into_iter()
        .take(CORPUS)
        .map(|(id, _)| id)
        .collect();
    for q in &queries {
        for t in targets(q) {
            if !corpus.contains(&t) {
                corpus.push(t);
            }
        }
    }
    let t = Instant::now();
    let docs = store.semantic_docs(&corpus).unwrap();
    info!(
        "{} documents read in {} ms",
        docs.len(),
        t.elapsed().as_millis()
    );
    let vecs = load_or_embed(&model, &docs);
    let ids: Vec<String> = docs.iter().map(|(id, _)| id.clone()).collect();
    let in_corpus: HashMap<&str, ()> = ids.iter().map(|i| (i.as_str(), ())).collect();

    let full: Vec<Vec<f32>> = vecs.iter().map(|v| unit(v, 768)).collect();
    let mut idx = VectorIndex::default();
    for (id, v) in ids.iter().zip(&vecs) {
        idx.upsert(id, "", &quantize(v));
    }
    let q256: Vec<Quantized> = vecs.iter().map(|v| quant_cut(v, 256)).collect();

    let names = [
        "fts (app search)",
        "vectors 768d f32",
        "vectors 768d int8",
        "vectors 256d int8",
        "hybrid rrf, top 100",
        "hybrid as shipped",
    ];
    let mut ranks: HashMap<&str, Vec<Option<usize>>> = HashMap::new();
    let (mut embed_ms, mut scan_ms, mut target_cos) = (0.0, 0.0, vec![]);
    for q in &queries {
        let (want, text) = (targets(q), q["query"].as_str().unwrap());
        let fts: Vec<String> = store
            .search(text, 100, false)
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .filter(|id| in_corpus.contains_key(id.as_str()))
            .collect();
        let t = Instant::now();
        let qv = model
            .embed(&[query_text(text)], Lane::Query)
            .unwrap()
            .remove(0);
        embed_ms += t.elapsed().as_secs_f64() * 1000.0;
        let qf = unit(&qv, 768);
        let top = |scores: Vec<f32>| -> Vec<String> {
            let mut s: Vec<(usize, f32)> = scores.into_iter().enumerate().collect();
            s.sort_by(|a, b| b.1.total_cmp(&a.1));
            s.iter().take(100).map(|(i, _)| ids[*i].clone()).collect()
        };
        let f768 = top(full
            .iter()
            .map(|d| d.iter().zip(&qf).map(|(a, b)| a * b).sum())
            .collect());
        let qq256 = quant_cut(&qv, 256);
        let i256 = top(q256.iter().map(|d| cosine(d, &qq256)).collect());
        let t = Instant::now();
        let near = idx.nearest(&quantize(&qv), 100, -1.0);
        scan_ms += t.elapsed().as_secs_f64() * 1000.0;
        if let Some((_, c)) = near.iter().find(|(id, _)| want.contains(id)) {
            target_cos.push(*c);
        }
        let shipped_near: Vec<String> = near
            .iter()
            .filter(|(_, c)| *c >= super::FLOOR)
            .take(super::NEIGHBOURS)
            .map(|(id, _)| id.clone())
            .collect();
        let i768: Vec<String> = near.into_iter().map(|(id, _)| id).collect();
        let hyb = rrf(&[fts.clone(), i768.clone()]);
        let shipped = rrf(&[fts.iter().take(30).cloned().collect(), shipped_near]);
        info!(
            "{:<55} fts {:<8} 768i8 {:<8} shipped {:?}",
            text.chars().take(55).collect::<String>(),
            format!("{:?}", rank(&fts, &want)),
            format!("{:?}", rank(&i768, &want)),
            rank(&shipped, &want)
        );
        for (name, list) in names
            .iter()
            .zip([&fts, &f768, &i768, &i256, &hyb, &shipped])
        {
            ranks.entry(name).or_default().push(rank(list, &want));
        }
    }
    info!(
        "n = {} queries, corpus {} threads",
        queries.len(),
        ids.len()
    );
    for name in names {
        report(name, &ranks[name]);
    }
    target_cos.sort_by(f32::total_cmp);
    info!(
        "query embedding {:.1} ms, {DIM}d scan of {} vectors {:.2} ms; target cosine min {:.2} median {:.2}",
        embed_ms / queries.len() as f64,
        idx.len(),
        scan_ms / queries.len() as f64,
        target_cos.first().copied().unwrap_or(0.0),
        target_cos.get(target_cos.len() / 2).copied().unwrap_or(0.0)
    );
    for junk in ["xqzvbt", "asdf", "hello", "the"] {
        let qv = model
            .embed(&[query_text(junk)], Lane::Query)
            .unwrap()
            .remove(0);
        let top = idx.nearest(&quantize(&qv), 1, -1.0);
        info!("best cosine for {junk:?}: {:.2}", top[0].1);
    }
    // A 100k-thread mailbox: the same vectors ten times over.
    let mut big = VectorIndex::default();
    for k in 0..10 {
        for (id, v) in ids.iter().zip(&vecs) {
            big.upsert(&format!("{id}-{k}"), "", &quantize(v));
        }
    }
    let qv = quantize(&vecs[0]);
    let t = Instant::now();
    for _ in 0..20 {
        big.nearest(&qv, 50, 0.0);
    }
    info!(
        "{DIM}d scan of {} vectors: {:.2} ms, RSS {:.0} MB",
        big.len(),
        t.elapsed().as_secs_f64() * 1000.0 / 20.0,
        rss_mb()
    );
}

/// A query's target threads (`thread`, or several in `threads`).
fn targets(q: &serde_json::Value) -> Vec<String> {
    match q.get("threads").and_then(|t| t.as_array()) {
        Some(a) => a
            .iter()
            .filter_map(|t| t.as_str().map(String::from))
            .collect(),
        None => vec![q["thread"].as_str().unwrap().to_string()],
    }
}

/// Indexing speed, memory, and how long a search waits while indexing runs.
#[test]
#[ignore = "needs the real model and a copy of a real mailbox"]
fn semantic_throughput() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    let store = Store::open(&PathBuf::from(env("MERMOZ_EVAL_DB")), &std::env::temp_dir()).unwrap();
    let model = LlamaEmbedder::load(&PathBuf::from(env("MERMOZ_EMBED_MODEL"))).unwrap();
    let ids: Vec<String> = store
        .semantic_sigs()
        .unwrap()
        .into_iter()
        .take(800)
        .map(|(id, _)| id)
        .collect();
    let docs = store.semantic_docs(&ids).unwrap();
    let t = Instant::now();
    let mut worst = 0f64;
    for chunk in docs.chunks(16) {
        let texts: Vec<String> = chunk.iter().map(|(_, d)| d.clone()).collect();
        std::thread::scope(|s| {
            let h = s.spawn(|| model.embed(&texts, Lane::Index).unwrap());
            std::thread::sleep(std::time::Duration::from_millis(30));
            let q = Instant::now();
            model
                .embed(&[query_text("ferry to the island")], Lane::Query)
                .unwrap();
            worst = worst.max(q.elapsed().as_secs_f64() * 1000.0);
            h.join().unwrap();
        });
    }
    info!(
        "{:.1} threads/s, a search during indexing waits up to {worst:.0} ms, RSS {:.0} MB",
        docs.len() as f64 / t.elapsed().as_secs_f64(),
        rss_mb()
    );
}
