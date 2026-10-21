//! The mail service behind an account. The sync engine asks the backend only for what
//! every service can give (a message's source, its headers); service-specific sync
//! and write-back live in their own modules.

use crate::error::{Error, Result};
use crate::gmail::{Gmail, decode_raw};
use std::sync::Arc;

/// An account's mail service client.
pub enum Backend {
    Gmail(Arc<Gmail>),
}

impl Backend {
    /// The account `provider` this backend serves.
    pub fn provider(&self) -> &'static str {
        match self {
            Backend::Gmail(_) => "gmail",
        }
    }

    /// The message exactly as received (RFC 822 bytes); `None` when the service sent
    /// nothing usable.
    pub async fn raw(&self, provider_id: &str) -> Result<Option<Vec<u8>>> {
        match self {
            Backend::Gmail(g) => Ok(g
                .get_raw(provider_id)
                .await?
                .raw
                .as_deref()
                .and_then(|r| decode_raw(r).ok())),
        }
    }

    /// Values of the named headers, in message order.
    pub async fn headers(
        &self,
        provider_id: &str,
        names: &[&str],
    ) -> Result<Vec<(String, String)>> {
        match self {
            Backend::Gmail(g) => g.headers(provider_id, names).await,
        }
    }

    /// The Gmail client, for Gmail-only calls.
    pub fn gmail(&self) -> Result<&Arc<Gmail>> {
        match self {
            Backend::Gmail(g) => Ok(g),
        }
    }
}

/// Error for a feature the account's mail service doesn't have.
pub fn unsupported(what: &str) -> Error {
    Error::InvalidParams(format!("{what} isn't available for this account"))
}
