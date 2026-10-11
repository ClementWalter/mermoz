//! "Sign in with Anthropic": the official `ant` CLI performs the browser OAuth login
//! and keeps tokens fresh; Mermoz gives it a private config directory and asks it for
//! a short-lived access token when calling the API.

use crate::error::{Error, Result};
use crate::paths;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// `ant` shipped inside Mermoz.app, then the usual install locations.
pub fn binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MERMOZ_ANT").map(PathBuf::from) {
        return Some(p);
    }
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("ant")));
    [
        bundled,
        Some("/opt/homebrew/bin/ant".into()),
        Some("/usr/local/bin/ant".into()),
    ]
    .into_iter()
    .flatten()
    .find(|p| p.exists())
}

fn config_dir() -> PathBuf {
    paths::home().join("anthropic")
}

fn command(args: &[&str]) -> Result<tokio::process::Command> {
    let bin =
        binary().ok_or_else(|| Error::other("Anthropic sign-in is not available in this build"))?;
    let mut c = tokio::process::Command::new(bin);
    c.args(args)
        .env("ANTHROPIC_CONFIG_DIR", config_dir())
        // A stray key in the environment would shadow the login profile.
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .kill_on_drop(true);
    Ok(c)
}

/// Opens the browser for the user to sign in; resolves when they finish (or after 5 minutes).
pub async fn login(profile: &str) -> Result<()> {
    std::fs::create_dir_all(config_dir())?;
    let out = command(&["auth", "login", "--profile", profile, "--timeout", "5m"])?
        .stdin(std::process::Stdio::null())
        .output()
        .await?;
    if out.status.success() {
        return Ok(());
    }
    // ant's stderr is written for terminals; the app shows one plain sentence.
    let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
    Err(Error::other(if err.contains("timed out") {
        "Sign-in timed out. Click the button again and finish in your browser."
    } else if err.contains("denied") || err.contains("access_denied") {
        "Sign-in was cancelled."
    } else {
        "Sign-in didn't complete. Please try again."
    }))
}

pub async fn logout(profile: &str) -> Result<()> {
    let _ = command(&["auth", "logout", "--profile", profile])?
        .output()
        .await?;
    Ok(())
}

static CACHE: Mutex<Option<HashMap<String, (String, Instant)>>> = Mutex::new(None);

/// Short-lived bearer token for a profile; `ant` refreshes it, we cache it for a few minutes.
pub async fn access_token(profile: &str) -> Result<String> {
    if let Some((t, at)) = CACHE
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(profile).cloned())
        && at.elapsed() < Duration::from_secs(240)
    {
        return Ok(t);
    }
    let out = command(&[
        "auth",
        "print-credentials",
        "--access-token",
        "--profile",
        profile,
    ])?
    .output()
    .await?;
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || token.is_empty() {
        return Err(Error::Upstream {
            status: 401,
            body: format!(
                "Anthropic sign-in expired: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        });
    }
    CACHE
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(profile.to_string(), (token.clone(), Instant::now()));
    Ok(token)
}
