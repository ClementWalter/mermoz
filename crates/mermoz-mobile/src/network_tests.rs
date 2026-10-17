//! Real HTTP exchanges exercise the mobile consent, sync and send paths against Gmail stubs.

use super::*;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use base64::Engine;
use rstest::{fixture, rstest};
use std::time::Duration;
use tempfile::TempDir;

type Requests = Arc<Mutex<Vec<(String, String, Value)>>>;

async fn ai_stub(State(requests): State<Requests>, Json(body): Json<Value>) -> Response {
    requests
        .lock()
        .unwrap()
        .push(("POST".into(), "/chat/completions".into(), body.clone()));
    if body["model"] == "refused" {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":"rate limited"})),
        )
            .into_response();
    }
    if body["model"] == "slow" {
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
    if body["stream"] == true {
        let text = "Alex needs confirmation.\n<actions>[{\"kind\":\"draft_reply\",\"label\":\"Review reply\",\"body\":\"Confirmed from AI.\"}]</actions>";
        let event = json!({"choices":[{"delta":{"content":text}}]});
        return (
            [("content-type", "text/event-stream")],
            format!("data: {event}\n\ndata: [DONE]\n\n"),
        )
            .into_response();
    }
    let schema = &body["response_format"]["json_schema"]["schema"]["properties"];
    let content = if schema.get("kind").is_some() {
        json!({"kind":"reply","needs_reply":1.0,"urgency":2.0,"automated":0.0,"due":null,"amount":null})
    } else {
        json!({"title":"Confirm the mobile beta","why":"Alex is waiting for confirmation.","draft":"Confirmed from AI.","summary":"Alex asks for confirmation.","points":[],"options":[]})
    };
    Json(json!({"choices":[{"message":{"content":content.to_string()}}]})).into_response()
}

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
    fn configure_ai(&self, model: &str) -> String {
        self.sync();
        let thread = self
            .engine
            .store
            .threads(&ThreadFilter {
                account_id: Some(self.account_id.clone()),
                limit: 1,
                ..Default::default()
            })
            .unwrap()
            .0
            .remove(0);
        self.engine
            .store
            .update_threads(
                std::slice::from_ref(&thread.id),
                &ThreadPatch {
                    status: Some("todo".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let configuration = json!({"provider":{"id":"mobile-ai","name":"Test provider","kind":"openai","base_url":self.engine.gmail_base,"model":model},
            "key":"mobile-test-secret-never-store","accounts":[self.account_id],"sorter":null});
        self.engine.dispatch(&json!({"method":"ai.configure","params":{"configuration":configuration.to_string()}}).to_string()).unwrap();
        thread.id
    }

    fn wait_for_ai(&self) {
        self.engine.runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(10), async {
                while self.engine.ai.status().running {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
        });
    }

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
            .route("/chat/completions", axum::routing::post(ai_stub))
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

#[rstest]
fn ai_prepares_a_real_feed_reply(connected: ConnectedMailbox) {
    let id = connected.configure_ai("test");
    connected
        .engine
        .dispatch(
            &json!({"method":"feed.refresh","params":{"ids":json!([id]).to_string()}}).to_string(),
        )
        .unwrap();
    connected.wait_for_ai();
    let feed = connected
        .engine
        .dispatch(r#"{"method":"feed.list","params":{}}"#)
        .unwrap();
    assert!(
        feed["sections"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|s| s["cards"].as_array().unwrap())
            .any(|c| c["texts"]["draft"] == "Confirmed from AI.")
    );
}

#[rstest]
fn ai_chat_persists_a_reviewable_reply(connected: ConnectedMailbox) {
    let id = connected.configure_ai("test");
    connected
        .engine
        .dispatch(
            &json!({"method":"chat.send","params":{"id":id,"question":"Help me reply"}})
                .to_string(),
        )
        .unwrap();
    connected.wait_for_ai();
    let chat = connected.engine.store.chat_messages(&id).unwrap();
    assert_eq!(
        chat.last().unwrap().actions[0].body.as_deref(),
        Some("Confirmed from AI.")
    );
}

#[rstest]
fn ai_chat_never_sends_a_reply(connected: ConnectedMailbox) {
    let id = connected.configure_ai("test");
    connected
        .engine
        .dispatch(
            &json!({"method":"chat.send","params":{"id":id,"question":"Send a reply"}}).to_string(),
        )
        .unwrap();
    connected.wait_for_ai();
    assert!(
        !connected
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(_, p, _)| p == "/messages/send")
    );
}

#[rstest]
fn ai_chat_omits_another_mailboxs_history(connected: ConnectedMailbox) {
    use mermoz_core::store::{IncomingMessage, SyncMode};
    let id = connected.configure_ai("test");
    let other = connected
        .engine
        .store
        .add_account("private@example.com", "gmail")
        .unwrap();
    connected.engine.store.ingest(&other.id, &other.email, &IncomingMessage {
        provider_id: "private-ask".into(), provider_thread_id: "private-thread".into(),
        label_ids: vec!["INBOX".into()], internal_date: now_ms() - 1000,
        parsed: mermoz_core::mime::parse(b"From: Alex <alex@example.com>\r\nTo: private@example.com\r\nSubject: Private question\r\n\r\nA question for your private mailbox.").unwrap(),
    }, SyncMode::Initial).unwrap();
    connected.engine.store.ingest(&other.id, &other.email, &IncomingMessage {
        provider_id: "private-answer".into(), provider_thread_id: "private-thread".into(),
        label_ids: vec!["SENT".into()], internal_date: now_ms(),
        parsed: mermoz_core::mime::parse(b"From: private@example.com\r\nTo: Alex <alex@example.com>\r\nSubject: Private question\r\n\r\nPRIVATE OTHER MAILBOX SECRET").unwrap(),
    }, SyncMode::Initial).unwrap();
    connected
        .engine
        .dispatch(
            &json!({"method":"chat.send","params":{"id":id,"question":"Help me reply"}})
                .to_string(),
        )
        .unwrap();
    connected.wait_for_ai();
    let requests = connected.requests.lock().unwrap();
    let request = requests
        .iter()
        .find(|(_, path, _)| path == "/chat/completions")
        .unwrap();
    assert!(
        !request
            .2
            .to_string()
            .contains("PRIVATE OTHER MAILBOX SECRET")
    );
}

#[rstest]
fn ai_never_reads_an_unapproved_mailbox(connected: ConnectedMailbox) {
    let id = connected.configure_ai("test");
    connected
        .engine
        .store
        .set_account_providers(&connected.account_id, &[])
        .unwrap();
    let other = connected
        .engine
        .store
        .add_account("other@example.com", "gmail")
        .unwrap();
    let config = json!({"provider":{"id":"mobile-ai","name":"Test","kind":"openai","base_url":connected.engine.gmail_base,"model":"test"},"key":"test","accounts":[other.id]});
    connected
        .engine
        .dispatch(
            &json!({"method":"ai.configure","params":{"configuration":config.to_string()}})
                .to_string(),
        )
        .unwrap();
    connected
        .engine
        .dispatch(
            &json!({"method":"feed.refresh","params":{"ids":json!([id]).to_string()}}).to_string(),
        )
        .unwrap();
    connected.wait_for_ai();
    assert!(
        !connected
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(_, p, _)| p == "/chat/completions")
    );
}

#[rstest]
fn provider_failures_remain_visible(connected: ConnectedMailbox) {
    let id = connected.configure_ai("refused");
    connected
        .engine
        .dispatch(
            &json!({"method":"feed.refresh","params":{"ids":json!([id]).to_string()}}).to_string(),
        )
        .unwrap();
    connected.wait_for_ai();
    assert!(connected.engine.ai.status().error.is_some());
}

#[rstest]
fn backgrounding_cancels_ai(connected: ConnectedMailbox) {
    let id = connected.configure_ai("slow");
    connected
        .engine
        .dispatch(
            &json!({"method":"feed.refresh","params":{"ids":json!([id]).to_string()}}).to_string(),
        )
        .unwrap();
    connected
        .engine
        .dispatch(r#"{"method":"sync.pause","params":{}}"#)
        .unwrap();
    assert!(!connected.engine.ai.status().running);
}

#[rstest]
fn ai_keys_are_not_restored_from_sqlite(connected: ConnectedMailbox) {
    connected.configure_ai("test");
    drop(connected.engine);
    let reopened = MobileEngine::open(connected._home.path()).unwrap();
    assert!(!reopened.ai.status().enabled);
}
