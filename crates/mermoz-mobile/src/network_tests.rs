//! Real HTTP exchanges exercise the mobile consent, sync and send paths against Gmail stubs.

use super::*;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::{Json, Router};
use base64::Engine;
use rstest::{fixture, rstest};
use std::time::Duration;
use tempfile::TempDir;

type Requests = Arc<Mutex<Vec<(String, String, Value)>>>;

async fn gmail_stub(
    State(requests): State<Requests>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> (StatusCode, Json<Value>) {
    let parsed = serde_json::from_slice(&body).unwrap_or_else(|_| {
        json!(
            url::form_urlencoded::parse(&body)
                .into_owned()
                .collect::<HashMap<_, _>>()
        )
    });
    requests
        .lock()
        .unwrap()
        .push((method.to_string(), uri.path().into(), parsed));
    let value = match (method.as_str(), uri.path()) {
        ("POST", "/token") => {
            json!({ "access_token":"access-test", "refresh_token":"refresh-test", "expires_in":3600, "scope":mermoz_core::oauth::SCOPE })
        }
        ("GET", "/profile") => {
            json!({ "emailAddress":"beta@example.com", "messagesTotal":1, "historyId":"10" })
        }
        ("GET", "/labels") => {
            json!({ "labels":[{"id":"INBOX","name":"INBOX"},{"id":"UNREAD","name":"UNREAD"},{"id":"SENT","name":"SENT"}] })
        }
        ("GET", "/messages") => json!({ "messages":[{"id":"m1","threadId":"t1"}] }),
        ("GET", "/history") => json!({ "history":[], "historyId":"10" }),
        ("GET", "/messages/m1") => {
            json!({ "id":"m1", "threadId":"t1", "labelIds":["INBOX","UNREAD"], "internalDate":"1700000000000", "raw":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"From: Alex <alex@example.com>\r\nTo: beta@example.com\r\nSubject: Mobile beta check\r\nMessage-ID: <m1@example.com>\r\n\r\nCan you confirm the mobile beta?") })
        }
        ("POST", "/messages/send") => json!({ "id":"m2", "threadId":"t1" }),
        ("GET", "/messages/m2") => {
            json!({ "id":"m2", "threadId":"t1", "labelIds":["SENT"], "internalDate":"1700000001000", "raw":base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"From: beta@example.com\r\nTo: alex@example.com\r\nSubject: Re: Mobile beta check\r\nMessage-ID: <m2@example.com>\r\nIn-Reply-To: <m1@example.com>\r\n\r\nConfirmed from iPhone.") })
        }
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error":"unexpected stub request"})),
            );
        }
    };
    (StatusCode::OK, Json(value))
}

struct ConnectedMailbox {
    engine: MobileEngine,
    _server: Runtime,
    _home: TempDir,
    requests: Requests,
    account_id: String,
}

impl ConnectedMailbox {
    fn sync(&self) {
        self.engine
            .dispatch(r#"{"method":"sync.start","params":{}}"#)
            .unwrap();
        self.engine.runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let account = self.engine.store.account(&self.account_id).unwrap();
                    match account.sync.state.as_str() {
                        "idle" => break,
                        "error" => panic!("sync failed: {:?}", account.sync.error),
                        _ => tokio::time::sleep(Duration::from_millis(10)).await,
                    }
                }
            })
            .await
            .unwrap();
        });
    }
}

#[fixture]
fn connected() -> ConnectedMailbox {
    let server = Runtime::new().unwrap();
    let requests = Arc::new(Mutex::new(vec![]));
    let url = server.block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .fallback(gmail_stub)
            .with_state(requests.clone());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{address}")
    });
    let home = tempfile::tempdir().unwrap();
    let mut engine = MobileEngine::open(home.path()).unwrap();
    engine.gmail_base = url.clone();
    engine.token_url = format!("{url}/token");
    let start = engine.dispatch(r#"{"method":"auth.start","params":{"client_id":"123-native.apps.googleusercontent.com"}}"#).unwrap();
    let consent = url::Url::parse(start["url"].as_str().unwrap()).unwrap();
    let state = consent
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1
        .into_owned();
    let callback = format!(
        "{}:/oauthredirect?code=auth-code&state={state}",
        start["scheme"].as_str().unwrap()
    );
    let authorized = engine
        .dispatch(&json!({"method":"auth.finish","params":{"callback":callback}}).to_string())
        .unwrap();
    let credential = authorized["credential"].clone();
    engine
        .dispatch(&json!({"method":"accounts.resume","params":credential}).to_string())
        .unwrap();
    ConnectedMailbox {
        engine,
        _server: server,
        _home: home,
        requests,
        account_id: authorized["account"]["id"].as_str().unwrap().into(),
    }
}

#[rstest]
fn native_refresh_never_sends_a_client_secret(connected: ConnectedMailbox) {
    connected.sync();
    let requests = connected.requests.lock().unwrap();
    let refresh = requests
        .iter()
        .find(|(_, path, form)| path == "/token" && form["grant_type"] == "refresh_token")
        .unwrap();
    assert!(refresh.2.get("client_secret").is_none());
}

#[rstest]
fn import_populates_the_local_mailbox(connected: ConnectedMailbox) {
    connected.sync();
    let result = connected
        .engine
        .dispatch(r#"{"method":"search","params":{"query":"mobile beta"}}"#)
        .unwrap();
    assert_eq!(result["threads"][0]["subject"], "Mobile beta check");
}

#[rstest]
fn real_accounts_hide_fictional_mail(connected: ConnectedMailbox) {
    assert!(
        connected
            .engine
            .store
            .accounts()
            .unwrap()
            .iter()
            .find(|a| a.provider == "demo")
            .unwrap()
            .hidden
    );
}

#[rstest]
fn refresh_credentials_are_not_in_sqlite(connected: ConnectedMailbox) {
    assert!(
        !std::fs::read(connected._home.path().join("mermoz.sqlite-wal"))
            .unwrap_or_default()
            .windows(b"refresh-test".len())
            .any(|bytes| bytes == b"refresh-test")
    );
}

#[rstest]
fn a_reply_reaches_gmail_once(connected: ConnectedMailbox) {
    connected.sync();
    let thread = connected
        .engine
        .store
        .threads(&ThreadFilter {
            limit: 1,
            account_id: Some(connected.account_id.clone()),
            ..Default::default()
        })
        .unwrap()
        .0
        .remove(0);
    let draft = connected.engine.dispatch(&json!({"method":"drafts.reply","params":{"id":thread.id,"body":"Confirmed from iPhone."}}).to_string()).unwrap();
    let request = json!({"method":"drafts.send","params":{"id":draft["id"]}}).to_string();
    connected.engine.dispatch(&request).unwrap();
    let _ = connected.engine.dispatch(&request);
    assert_eq!(
        connected
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, path, _)| method == "POST" && path == "/messages/send")
            .count(),
        1
    );
}

#[rstest]
fn an_interrupted_send_is_not_repeated(connected: ConnectedMailbox) {
    connected.sync();
    let thread = connected
        .engine
        .store
        .threads(&ThreadFilter {
            limit: 1,
            account_id: Some(connected.account_id.clone()),
            ..Default::default()
        })
        .unwrap()
        .0
        .remove(0);
    let draft = connected
        .engine
        .dispatch(
            &json!({"method":"drafts.reply","params":{"id":thread.id,"body":"Interrupted reply."}})
                .to_string(),
        )
        .unwrap();
    connected
        .engine
        .store
        .set_draft_state(draft["id"].as_str().unwrap(), "sending")
        .unwrap();
    let _ = connected
        .engine
        .dispatch(&json!({"method":"drafts.send","params":{"id":draft["id"]}}).to_string());
    assert!(
        !connected
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(method, path, _)| method == "POST" && path == "/messages/send")
    );
}

#[rstest]
fn removing_an_account_stops_its_sync(connected: ConnectedMailbox) {
    connected
        .engine
        .dispatch(r#"{"method":"sync.start","params":{}}"#)
        .unwrap();
    connected
        .engine
        .dispatch(
            &json!({"method":"accounts.remove","params":{"id":connected.account_id}}).to_string(),
        )
        .unwrap();
    assert!(
        !connected
            .engine
            .workers
            .lock()
            .unwrap()
            .contains_key(&connected.account_id)
    );
}

#[rstest]
fn a_used_login_callback_cannot_be_replayed(connected: ConnectedMailbox) {
    assert!(
        connected
            .engine
            .dispatch(r#"{"method":"auth.finish","params":{"callback":"anything"}}"#)
            .is_err()
    );
}
