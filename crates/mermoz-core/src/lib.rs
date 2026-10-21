//! Mermoz core: the local mail store, the Gmail sync engine and the issue model.
//!
//! The desktop daemon and embedded mobile engine share these storage and workflow rules.

pub mod agent;
pub mod ant;
pub mod auth;
pub mod chat;
pub mod chatgpt;
pub mod compose;
pub mod conversation;
pub mod decorative;
pub mod demo;
pub mod demo_account;
pub mod drive;
pub mod drive_cache;
pub mod dsn;
pub mod error;
pub mod feed;
pub mod gmail;
pub mod inline;
pub mod jev;
pub mod mime;
pub mod model;
pub mod newsletter;
pub mod oauth;
pub mod paths;
pub mod plan;
pub mod provider;
pub mod quote;
pub mod reading;
pub mod reply;
pub mod sanitize;
pub mod semantic;
pub mod store;
pub mod sync;
pub mod tokens;

pub use error::{Error, Result};
