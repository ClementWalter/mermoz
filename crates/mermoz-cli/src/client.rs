//! Minimal synchronous JSON-RPC client for the mermozd socket; starts the
//! daemon on demand so agents work even when the app is closed.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub fn home() -> PathBuf {
    if let Some(h) = std::env::var_os("MERMOZ_HOME") {
        return PathBuf::from(h);
    }
    let base = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("Library/Application Support/Mermoz")
}

#[derive(Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

pub const DAEMON_UNREACHABLE: i64 = -1;

pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next_id: u64,
}

/// The daemon binary shipped next to this CLI (inside Mermoz.app/Contents/Resources).
fn sibling_daemon() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let p = exe.parent()?.join("mermozd");
    p.exists().then_some(p)
}

fn spawn_daemon() -> bool {
    let Some(bin) = std::env::var_os("MERMOZ_DAEMON")
        .map(PathBuf::from)
        .or_else(sibling_daemon)
    else {
        return false;
    };
    let _ = std::fs::create_dir_all(home());
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home().join("mermozd.log"))
        .ok();
    let mut cmd = std::process::Command::new(bin);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    match log {
        Some(f) => cmd.stderr(f),
        None => cmd.stderr(std::process::Stdio::null()),
    };
    cmd.spawn().is_ok()
}

impl Client {
    pub fn connect() -> Result<Self, RpcError> {
        let sock = home().join("mermozd.sock");
        let stream = match UnixStream::connect(&sock) {
            Ok(s) => s,
            Err(_) => {
                if !spawn_daemon() {
                    return Err(RpcError {
                        code: DAEMON_UNREACHABLE,
                        message:
                            "Mermoz isn't running. Open Mermoz.app (or start mermozd) and retry."
                                .into(),
                        data: None,
                    });
                }
                let deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    if let Ok(s) = UnixStream::connect(&sock) {
                        break s;
                    }
                    if Instant::now() > deadline {
                        return Err(RpcError { code: DAEMON_UNREACHABLE, message: "Started mermozd but it did not come up within 3s; see mermozd.log in the Mermoz data folder.".into(), data: None });
                    }
                    std::thread::sleep(Duration::from_millis(30));
                }
            }
        };
        let writer = stream.try_clone().map_err(|e| RpcError {
            code: DAEMON_UNREACHABLE,
            message: e.to_string(),
            data: None,
        })?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
            next_id: 1,
        })
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let io = |e: std::io::Error| RpcError {
            code: DAEMON_UNREACHABLE,
            message: format!("lost connection to mermozd: {e}"),
            data: None,
        };
        writeln!(self.writer, "{req}").map_err(io)?;
        loop {
            let mut line = String::new();
            if self.reader.read_line(&mut line).map_err(io)? == 0 {
                return Err(RpcError {
                    code: DAEMON_UNREACHABLE,
                    message: "mermozd closed the connection".into(),
                    data: None,
                });
            }
            let v: Value = serde_json::from_str(&line).map_err(|e| RpcError {
                code: -32700,
                message: e.to_string(),
                data: None,
            })?;
            if v.get("id").and_then(|i| i.as_u64()) != Some(id) {
                continue;
            }
            if let Some(e) = v.get("error") {
                return Err(RpcError {
                    code: e.get("code").and_then(|c| c.as_i64()).unwrap_or(-32000),
                    message: e
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("error")
                        .to_string(),
                    data: e.get("data").cloned(),
                });
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}
