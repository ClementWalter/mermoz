//! Mermoz core: the local mail store, the Gmail sync engine and the issue model.
//!
//! Everything here runs inside `mermozd`; clients only see it through the
//! JSON-RPC protocol described in `docs/protocol.md`.

pub mod agent;
pub mod ant;
pub mod auth;
pub mod chatgpt;
pub mod compose;
pub mod error;
pub mod gmail;
pub mod jev;
pub mod mime;
pub mod model;
pub mod oauth;
pub mod paths;
pub mod quote;
pub mod sanitize;
pub mod store;
pub mod sync;
pub mod tokens;

pub use error::{Error, Result};
