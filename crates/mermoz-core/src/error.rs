//! Error type shared by the core; variants map 1:1 to protocol error codes.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid params: {0}")]
    InvalidParams(String),
    #[error("upstream error {status}: {body}")]
    Upstream { status: u16, body: String },
    /// HTTP 402: the AI account has no credits left; nothing is retried until topped up.
    /// Carries the provider's name, for the message.
    #[error("Sorting paused: your {0} credits ran out")]
    OutOfCredits(&'static str),
    /// HTTP 429; `retry_after` is the wait the server asked for, when it said.
    #[error("rate limited")]
    RateLimited {
        retry_after: Option<std::time::Duration>,
    },
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("http error: {}", chain(.0))]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// The user said no on a consent screen (Google's `error=` on the redirect).
    #[error("authorization declined: {0}")]
    Declined(String),
    /// Google no longer accepts the account's sign-in (`invalid_grant` on refresh:
    /// access removed, password changed, or the weekly expiry of an app in testing).
    /// Nothing works for the account until the user signs in again.
    #[error("Google asks you to sign in again")]
    SignInExpired,
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }
}

/// reqwest's Display hides the cause (TLS, reset, timeout); logs need the whole chain.
fn chain(e: &reqwest::Error) -> String {
    let mut out = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        out.push_str(": ");
        out.push_str(&s.to_string());
        src = s.source();
    }
    out
}
