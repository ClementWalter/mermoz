//! Native Google authorization with PKCE and a single-use, expiring callback.

use base64::Engine;
use mermoz_core::oauth::{ClientConfig, SCOPE};
use mermoz_core::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::time::{Duration, Instant};

#[derive(Clone, Deserialize, Serialize)]
pub struct Credential {
    pub account_id: String,
    pub client_id: String,
    pub refresh_token: String,
}

pub struct Login {
    pub client_id: String,
    pub redirect: String,
    pub auth_url: String,
    state: String,
    verifier: String,
    created: Instant,
}

#[derive(Deserialize)]
pub struct Grant {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: Option<u64>,
    pub scope: String,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn client(client_id: &str) -> Result<ClientConfig> {
    let prefix = client_id
        .strip_suffix(".apps.googleusercontent.com")
        .unwrap_or("");
    if prefix.is_empty()
        || !prefix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(Error::InvalidParams(
            "A Google iOS client ID is required".into(),
        ));
    }
    Ok(ClientConfig::with_ids(client_id.into(), String::new()))
}

impl Login {
    pub fn new(client_id: String) -> Result<Self> {
        let cfg = client(&client_id)?;
        let redirect = format!(
            "{}:/oauthredirect",
            client_id.split('.').rev().collect::<Vec<_>>().join(".")
        );
        let state = b64(&rand::random::<[u8; 32]>());
        let verifier = b64(&rand::random::<[u8; 32]>());
        let challenge = b64(&sha2::Sha256::digest(verifier.as_bytes()));
        let mut url = url::Url::parse(&cfg.auth_url).map_err(|e| Error::other(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("response_type", "code")
            .append_pair("scope", SCOPE)
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent")
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        Ok(Self {
            client_id,
            redirect,
            auth_url: url.into(),
            state,
            verifier,
            created: Instant::now(),
        })
    }

    fn code(&self, callback: &str) -> Result<String> {
        if self.created.elapsed() > Duration::from_secs(600) {
            return Err(Error::other("Sign-in expired. Please try again."));
        }
        let callback =
            url::Url::parse(callback).map_err(|_| Error::other("Invalid sign-in callback"))?;
        let mut target = callback.clone();
        target.set_query(None);
        if target.as_str() != self.redirect {
            return Err(Error::other("Sign-in returned to an unexpected address"));
        }
        let unique = |name: &str| -> Result<Option<String>> {
            let values: Vec<_> = callback
                .query_pairs()
                .filter(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
                .collect();
            if values.len() > 1 {
                return Err(Error::other("Ambiguous sign-in callback"));
            }
            Ok(values.into_iter().next())
        };
        if unique("state")?.as_deref() != Some(&self.state) {
            return Err(Error::other("Sign-in state did not match"));
        }
        if unique("error")?.is_some() {
            return Err(Error::Declined("Sign-in was cancelled".into()));
        }
        unique("code")?
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::other("Sign-in returned no code"))
    }

    pub async fn finish(self, callback: &str, endpoint: &str) -> Result<(String, Grant)> {
        self.exchange(callback, endpoint).await
    }

    async fn exchange(self, callback: &str, endpoint: &str) -> Result<(String, Grant)> {
        let code = self.code(callback)?;
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?
            .post(endpoint)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("code", &code),
                ("redirect_uri", &self.redirect),
                ("code_verifier", &self.verifier),
                ("grant_type", "authorization_code"),
            ])
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(Error::other(
                "Google couldn't complete sign-in. Please try again.",
            ));
        }
        let grant: Grant = response.json().await?;
        if !grant.scope.split_whitespace().any(|s| s == SCOPE) || grant.refresh_token.is_empty() {
            return Err(Error::other("Allow Gmail access to connect your mailbox."));
        }
        Ok((self.client_id, grant))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::missing_state("?code=abc")]
    #[case::wrong_state("?code=abc&state=wrong")]
    #[case::wrong_origin("/wrong?code=abc")]
    fn unbound_callbacks_are_rejected(#[case] suffix: &str) {
        let login = Login::new("123-native.apps.googleusercontent.com".into()).unwrap();
        assert!(login.code(&format!("{}{suffix}", login.redirect)).is_err());
    }

    #[test]
    fn duplicate_state_is_rejected() {
        let login = Login::new("123-native.apps.googleusercontent.com".into()).unwrap();
        assert!(
            login
                .code(&format!(
                    "{}?code=abc&state={}&state={}",
                    login.redirect, login.state, login.state
                ))
                .is_err()
        );
    }

    #[test]
    fn a_bound_callback_returns_its_code() {
        let login = Login::new("123-native.apps.googleusercontent.com".into()).unwrap();
        assert_eq!(
            login
                .code(&format!(
                    "{}?code=abc&state={}",
                    login.redirect, login.state
                ))
                .unwrap(),
            "abc"
        );
    }

    #[test]
    fn the_consent_url_uses_pkce() {
        let login = Login::new("123-native.apps.googleusercontent.com".into()).unwrap();
        let url = url::Url::parse(&login.auth_url).unwrap();
        assert_eq!(
            url.query_pairs()
                .find(|(key, _)| key == "code_challenge")
                .unwrap()
                .1,
            b64(&sha2::Sha256::digest(login.verifier.as_bytes()))
        );
    }

    #[test]
    fn expired_callbacks_are_rejected() {
        let mut login = Login::new("123-native.apps.googleusercontent.com".into()).unwrap();
        login.created = Instant::now() - Duration::from_secs(601);
        assert!(
            login
                .code(&format!(
                    "{}?code=abc&state={}",
                    login.redirect, login.state
                ))
                .is_err()
        );
    }
}
