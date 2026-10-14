//! EmbeddingGemma on llama.cpp, in process, on the GPU (Metal).
//!
//! One model, two worker threads, each with its own context: searches at normal
//! priority, indexing at utility priority, so a search never queues behind indexing
//! and indexing yields the CPU and GPU to the app.
//!
//! Each pass encodes one text. This llama.cpp reserves a vocabulary-wide output row
//! (1 MB) per token of the largest pass even for an embedding model, so one text of at
//! most [`MAX_TOKENS`] keeps that buffer near 0.5 GB per context; packing 8 texts per
//! pass was only 20% faster and took 3.9 GB.

use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::{LlamaContextParams, LlamaPoolingType};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use mermoz_core::semantic::{Embedder, Lane};
use mermoz_core::{Error, Result};
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};

/// Tokens read per text: a thread document is ~1,600 characters, about 400 tokens.
const MAX_TOKENS: usize = 512;

/// llama.cpp's backend can start once per process.
static BACKEND: OnceLock<std::result::Result<LlamaBackend, String>> = OnceLock::new();

/// Creating two contexts on one model at the same time is unsound in llama.cpp.
static NEW_CONTEXT: Mutex<()> = Mutex::new(());

type Reply = Sender<Result<Vec<Vec<f32>>>>;

/// Texts to embed and where to send their vectors.
struct Job(Vec<String>, Reply);

/// EmbeddingGemma loaded from a GGUF file, behind the [`Embedder`] trait. Dropping it
/// stops both workers and frees the model: Metal aborts the process at exit if GPU
/// buffers are still alive.
pub struct LlamaEmbedder {
    query: Mutex<Option<Sender<Job>>>,
    index: Mutex<Option<Sender<Job>>>,
    workers: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

fn backend() -> Result<&'static LlamaBackend> {
    BACKEND
        .get_or_init(|| {
            llama_cpp_2::send_logs_to_tracing(
                llama_cpp_2::LogOptions::default().with_logs_enabled(false),
            );
            LlamaBackend::init().map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| Error::other(format!("llama.cpp: {e}")))
}

impl LlamaEmbedder {
    /// Loads the model and starts both workers; fails if the file isn't a usable model.
    pub fn load(path: &Path) -> Result<Self> {
        let backend = backend()?;
        let params = LlamaModelParams::default().with_n_gpu_layers(999);
        let model = LlamaModel::load_from_file(backend, path, &params)
            .map_err(|e| Error::other(format!("loading the search model: {e}")))?;
        let model = Arc::new(model);
        let (query, q) = start(model.clone(), Lane::Query)?;
        let (index, i) = start(model, Lane::Index)?;
        Ok(Self {
            query: Mutex::new(Some(query)),
            index: Mutex::new(Some(index)),
            workers: Mutex::new(vec![q, i]),
        })
    }
}

impl Drop for LlamaEmbedder {
    fn drop(&mut self) {
        self.query.lock().unwrap().take();
        self.index.lock().unwrap().take();
        for w in self.workers.lock().unwrap().drain(..) {
            let _ = w.join();
        }
    }
}

impl Embedder for LlamaEmbedder {
    fn embed(&self, texts: &[String], lane: Lane) -> Result<Vec<Vec<f32>>> {
        let (tx, rx) = channel();
        let jobs = match lane {
            Lane::Query => &self.query,
            Lane::Index => &self.index,
        };
        jobs.lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| Error::other("the search model stopped"))?
            .send(Job(texts.to_vec(), tx))
            .map_err(|_| Error::other("the search model stopped"))?;
        rx.recv()
            .map_err(|_| Error::other("the search model stopped"))?
    }
}

type Worker = (Sender<Job>, std::thread::JoinHandle<()>);

/// Starts a worker thread with its own context; returns once the context exists.
fn start(model: Arc<LlamaModel>, lane: Lane) -> Result<Worker> {
    let (jobs, rx) = channel::<Job>();
    let (ready_tx, ready) = channel::<Result<()>>();
    let handle = std::thread::Builder::new()
        .name(format!("embed-{lane:?}").to_lowercase())
        .spawn(move || {
            if lane == Lane::Index {
                // Utility QoS: macOS runs it after anything the user is waiting on.
                unsafe {
                    libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0)
                };
            }
            let n_ctx = MAX_TOKENS as u32;
            let threads = std::thread::available_parallelism().map_or(4, |n| n.get() as i32 / 2);
            let params = LlamaContextParams::default()
                .with_embeddings(true)
                .with_pooling_type(LlamaPoolingType::Mean)
                .with_n_ctx(NonZeroU32::new(n_ctx))
                .with_n_batch(n_ctx)
                .with_n_ubatch(n_ctx)
                .with_n_seq_max(1)
                .with_n_threads(threads)
                .with_n_threads_batch(threads);
            let ctx = {
                let _one = NEW_CONTEXT.lock().unwrap();
                backend().and_then(|b| {
                    model
                        .new_context(b, params)
                        .map_err(|e| Error::other(format!("search model context: {e}")))
                })
            };
            let mut ctx = match ctx {
                Ok(c) => {
                    let _ = ready_tx.send(Ok(()));
                    c
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            serve(&model, &mut ctx, rx);
        })
        .map_err(|e| Error::other(e.to_string()))?;
    ready
        .recv()
        .map_err(|_| Error::other("the search model stopped"))??;
    Ok((jobs, handle))
}

fn serve(model: &LlamaModel, ctx: &mut LlamaContext, rx: Receiver<Job>) {
    while let Ok(Job(texts, reply)) = rx.recv() {
        let out = texts
            .iter()
            .map(|t| embed_one(model, ctx, t))
            .collect::<Result<Vec<_>>>();
        let _ = reply.send(out);
    }
}

/// One text's mean-pooled vector.
fn embed_one(model: &LlamaModel, ctx: &mut LlamaContext, text: &str) -> Result<Vec<f32>> {
    let mut tokens = model.vocab().tokenize(text.as_bytes(), true, false);
    tokens.truncate(MAX_TOKENS);
    let mut batch = LlamaBatch::new(tokens.len().max(1), 1);
    batch
        .add_sequence(&tokens, 0, true)
        .map_err(|e| Error::other(format!("search model batch: {e}")))?;
    ctx.clear_kv_cache();
    ctx.decode(&mut batch)
        .map_err(|e| Error::other(format!("search model: {e}")))?;
    ctx.embeddings_seq_ith(0)
        .map(<[f32]>::to_vec)
        .map_err(|e| Error::other(format!("search model output: {e}")))
}

#[cfg(test)]
mod tests {
    //! Needs the real model: `MERMOZ_EMBED_MODEL=/path/to/embeddinggemma-300M-Q8_0.gguf
    //! cargo test --release -p mermozd llama -- --ignored`.
    use super::*;
    use mermoz_core::semantic::{cosine, quantize};
    use rstest::{fixture, rstest};

    #[fixture]
    fn model() -> LlamaEmbedder {
        let path = std::env::var("MERMOZ_EMBED_MODEL").expect("MERMOZ_EMBED_MODEL");
        LlamaEmbedder::load(Path::new(&path)).unwrap()
    }

    fn vec(m: &LlamaEmbedder, t: &str, lane: Lane) -> Vec<f32> {
        m.embed(&[t.to_string()], lane).unwrap().remove(0)
    }

    #[rstest]
    #[ignore = "needs the real model"]
    fn a_vector_has_768_dimensions(model: LlamaEmbedder) {
        assert_eq!(vec(&model, "hello", Lane::Query).len(), 768);
    }

    #[rstest]
    #[ignore = "needs the real model"]
    fn both_lanes_agree(model: LlamaEmbedder) {
        let t = "title: Ferry | text: From Nora. The ferry leaves at noon.";
        let (a, b) = (
            quantize(&vec(&model, t, Lane::Query)),
            quantize(&vec(&model, t, Lane::Index)),
        );
        assert!(cosine(&a, &b) > 0.99);
    }

    #[rstest]
    #[ignore = "needs the real model"]
    fn a_query_is_closer_to_its_topic(model: LlamaEmbedder) {
        let q = quantize(&vec(
            &model,
            "task: search result | query: boat timetable",
            Lane::Query,
        ));
        let docs = model
            .embed(
                &[
                    "title: Ferry | text: From Nora. The ferry leaves at noon from pier 3.".into(),
                    "title: Invoice | text: From Billing. Your invoice for March is attached."
                        .into(),
                ],
                Lane::Index,
            )
            .unwrap();
        assert!(cosine(&q, &quantize(&docs[0])) > cosine(&q, &quantize(&docs[1])));
    }
}
