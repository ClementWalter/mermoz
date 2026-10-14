//! Fetches the search model on first use: from pinned URLs (a mirror after the main
//! host), resuming a partial file, and accepted only if its SHA-256 matches. The
//! finished file appears under its final name only once verified.

use mermoz_core::{Error, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// A file to fetch: where from (tried in order), and what it must hash to.
#[derive(Debug, Clone)]
pub struct Pinned {
    pub name: &'static str,
    pub urls: &'static [&'static str],
    pub sha256: &'static str,
    pub size: u64,
}

/// EmbeddingGemma 300M, Q8_0 (gemma-embedding architecture), at a pinned revision.
pub const MODEL: Pinned = Pinned {
    name: "embeddinggemma-300M-Q8_0.gguf",
    urls: &[
        "https://huggingface.co/ggml-org/embeddinggemma-300M-GGUF/resolve/0f741b5a6585bd53aeb15cd1372c56f2a0f65e12/embeddinggemma-300M-Q8_0.gguf",
        "https://modelscope.cn/models/ggml-org/embeddinggemma-300M-GGUF/resolve/master/embeddinggemma-300M-Q8_0.gguf",
    ],
    sha256: "b5ce9d77a3fc4b3b39ccb5643c36777911cc4eb46a66962eadfa3f5f60490d63",
    size: 333_590_944,
};

fn sha256_of(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The model's path in `dir`, fetching it first if needed. `progress(done, total)` is
/// called as bytes arrive.
pub async fn fetch(
    http: &reqwest::Client,
    file: &Pinned,
    dir: &Path,
    progress: impl Fn(u64, u64),
) -> Result<PathBuf> {
    let done = dir.join(file.name);
    if done.exists() {
        return Ok(done);
    }
    std::fs::create_dir_all(dir)?;
    let part = dir.join(format!("{}.part", file.name));
    let mut last = Error::other("no download source");
    for url in file.urls {
        match fetch_from(http, url, file, &part, &progress).await {
            Ok(()) => {}
            Err(e) => {
                tracing::info!(url, "search model download stopped: {e}");
                last = e;
                continue;
            }
        }
        let part2 = part.clone();
        let hash = tokio::task::spawn_blocking(move || sha256_of(&part2))
            .await
            .map_err(|e| Error::other(e.to_string()))??;
        if hash == file.sha256 {
            std::fs::rename(&part, &done)?;
            return Ok(done);
        }
        tracing::warn!(url, %hash, "search model hash mismatch; discarded");
        let _ = std::fs::remove_file(&part);
        last = Error::other("the downloaded search model was damaged");
    }
    Err(last)
}

/// Appends the rest of the file to `part` (from scratch if the server can't resume).
async fn fetch_from(
    http: &reqwest::Client,
    url: &str,
    file: &Pinned,
    part: &Path,
    progress: &impl Fn(u64, u64),
) -> Result<()> {
    let have = std::fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if have == file.size {
        return Ok(());
    }
    let mut req = http.get(url);
    if have > 0 && have < file.size {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut resp = req.send().await?;
    let status = resp.status().as_u16();
    let mut out = match status {
        206 => std::fs::OpenOptions::new().append(true).open(part)?,
        200 => std::fs::File::create(part)?,
        _ => {
            return Err(Error::Upstream {
                status,
                body: String::new(),
            });
        }
    };
    let mut got = if status == 206 { have } else { 0 };
    while let Some(chunk) = resp.chunk().await? {
        out.write_all(&chunk)?;
        got += chunk.len() as u64;
        progress(got, file.size);
        if got > file.size {
            return Err(Error::other("the search model is larger than expected"));
        }
    }
    out.flush()?;
    if got < file.size {
        return Err(Error::other("the search model download was cut short"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use rstest::{fixture, rstest};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    const BODY: &[u8] = b"a tiny fictional model file, a few dozen bytes long";

    fn sha(b: &[u8]) -> &'static str {
        let h: String = Sha256::digest(b)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Box::leak(h.into_boxed_str())
    }

    /// Serves BODY with Range support; counts the bytes it sent.
    struct Server {
        base: String,
        sent: Arc<AtomicU64>,
    }

    async fn serve(State(sent): State<Arc<AtomicU64>>, h: HeaderMap) -> impl IntoResponse {
        let from = h
            .get("range")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("bytes="))
            .and_then(|v| v.trim_end_matches('-').parse::<usize>().ok());
        let (code, body) = match from {
            Some(f) => (StatusCode::PARTIAL_CONTENT, BODY[f..].to_vec()),
            None => (StatusCode::OK, BODY.to_vec()),
        };
        sent.fetch_add(body.len() as u64, Ordering::SeqCst);
        (code, body)
    }

    #[fixture]
    fn server() -> Server {
        let sent = Arc::new(AtomicU64::new(0));
        let app = Router::new()
            .route("/model", get(serve))
            .route("/gone", get(|| async { StatusCode::NOT_FOUND }))
            .with_state(sent.clone());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async move {
                    let l = tokio::net::TcpListener::from_std(listener).unwrap();
                    axum::serve(l, app).await.unwrap();
                })
        });
        Server { base, sent }
    }

    fn pinned(urls: Vec<String>, sha256: &'static str) -> Pinned {
        Pinned {
            name: "m.gguf",
            urls: Box::leak(
                urls.into_iter()
                    .map(|u| &*Box::leak(u.into_boxed_str()))
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
            sha256,
            size: BODY.len() as u64,
        }
    }

    fn run(file: &Pinned, dir: &Path) -> Result<PathBuf> {
        tokio::runtime::Runtime::new().unwrap().block_on(fetch(
            &reqwest::Client::new(),
            file,
            dir,
            |_, _| {},
        ))
    }

    #[rstest]
    fn fetch_saves_the_verified_file(server: Server) {
        let dir = tempfile::tempdir().unwrap();
        let f = pinned(vec![format!("{}/model", server.base)], sha(BODY));
        assert_eq!(std::fs::read(run(&f, dir.path()).unwrap()).unwrap(), BODY);
    }

    #[rstest]
    fn fetch_resumes_a_partial_file(server: Server) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("m.gguf.part"), &BODY[..40]).unwrap();
        let f = pinned(vec![format!("{}/model", server.base)], sha(BODY));
        run(&f, dir.path()).unwrap();
        assert_eq!(server.sent.load(Ordering::SeqCst), BODY.len() as u64 - 40);
    }

    #[rstest]
    fn fetch_rejects_a_wrong_hash(server: Server) {
        let dir = tempfile::tempdir().unwrap();
        let f = pinned(vec![format!("{}/model", server.base)], sha(b"other"));
        assert!(run(&f, dir.path()).is_err());
    }

    #[rstest]
    fn a_wrong_hash_leaves_no_file(server: Server) {
        let dir = tempfile::tempdir().unwrap();
        let f = pinned(vec![format!("{}/model", server.base)], sha(b"other"));
        let _ = run(&f, dir.path());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[rstest]
    fn fetch_falls_back_to_the_mirror(server: Server) {
        let dir = tempfile::tempdir().unwrap();
        let f = pinned(
            vec![
                format!("{}/gone", server.base),
                format!("{}/model", server.base),
            ],
            sha(BODY),
        );
        assert!(run(&f, dir.path()).is_ok());
    }

    #[rstest]
    fn fetch_skips_a_file_already_there(server: Server) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("m.gguf"), BODY).unwrap();
        let f = pinned(vec![format!("{}/model", server.base)], sha(BODY));
        run(&f, dir.path()).unwrap();
        assert_eq!(server.sent.load(Ordering::SeqCst), 0);
    }
}
