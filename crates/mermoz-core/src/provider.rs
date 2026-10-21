//! The mail service behind an account. The sync engine asks the backend only for what
//! every service can give (a message's source, its headers); service-specific sync
//! and write-back live in their own modules (`sync` for Gmail, `sync::outlook`).

use crate::error::{Error, Result};
use crate::gmail::{Gmail, decode_raw};
use crate::outlook::Outlook;
use std::sync::Arc;

/// An account's mail service client.
pub enum Backend {
    Gmail(Arc<Gmail>),
    /// Microsoft Outlook (personal or Microsoft 365) through Microsoft Graph.
    Outlook(Arc<Outlook>),
}

impl Backend {
    /// The account `provider` this backend serves.
    pub fn provider(&self) -> &'static str {
        match self {
            Backend::Gmail(_) => "gmail",
            Backend::Outlook(_) => "outlook",
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
            Backend::Outlook(o) => Ok(Some(o.mime(provider_id).await?)),
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
            // Graph has no header-only fetch for raw headers: read them off the source.
            Backend::Outlook(o) => Ok(header_values(&o.mime(provider_id).await?, names)),
        }
    }

    /// The Gmail client, for Gmail-only calls.
    pub fn gmail(&self) -> Result<&Arc<Gmail>> {
        match self {
            Backend::Gmail(g) => Ok(g),
            Backend::Outlook(_) => Err(unsupported("This")),
        }
    }
}

/// Error for a feature the account's mail service doesn't have.
pub fn unsupported(what: &str) -> Error {
    Error::InvalidParams(format!("{what} isn't available for this account"))
}

/// The named headers of an RFC 822 message (unfolded), in message order.
pub fn header_values(raw: &[u8], names: &[&str]) -> Vec<(String, String)> {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n"))
        .unwrap_or(raw.len());
    let head = String::from_utf8_lossy(&raw[..end]);
    let mut lines: Vec<String> = vec![];
    for line in head.lines() {
        match lines.last_mut() {
            Some(last) if line.starts_with([' ', '\t']) => {
                last.push(' ');
                last.push_str(line.trim());
            }
            _ => lines.push(line.to_string()),
        }
    }
    lines
        .iter()
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| names.iter().any(|n| n.eq_ignore_ascii_case(k.trim())))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const RAW: &[u8] = b"From: a@example.com\r\nReply-To: Desk\r\n <desk@example.com>\r\nSubject: Hi\r\n\r\nReply-To: not a header\r\n";

    #[rstest]
    fn folded_headers_are_unfolded() {
        assert_eq!(
            header_values(RAW, &["reply-to"]),
            vec![(
                "Reply-To".to_string(),
                "Desk <desk@example.com>".to_string()
            )]
        );
    }

    #[rstest]
    fn other_headers_are_left_out() {
        assert_eq!(header_values(RAW, &["X-Missing"]), vec![]);
    }
}
