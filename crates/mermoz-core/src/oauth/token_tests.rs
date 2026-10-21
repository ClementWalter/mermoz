//! How a [`TokenSource`] reacts when Google refuses its refresh token, against a stub
//! token endpoint that counts the requests it gets.

use super::*;
use rstest::rstest;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use tokio::net::TcpListener;

const REFUSED: (u16, &str) = (
    400,
    r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
);
const GRANTED: (u16, &str) = (200, r#"{"access_token":"at","expires_in":3600}"#);

/// A token endpoint answering `replies` in order (the last one repeats); returns its URL
/// and the number of requests it served.
async fn token_stub(replies: Vec<(u16, &'static str)>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/token", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let (status, body) = replies[n.min(replies.len() - 1)];
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await;
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(reply.as_bytes()).await;
        }
    });
    (url, hits)
}

fn source(url: &str) -> TokenSource {
    let mut cfg = ClientConfig::with_ids("cid".into(), "secret".into());
    cfg.token_url = url.into();
    TokenSource::new(cfg, "refresh".into())
}

#[rstest]
#[case::revoked(
    400,
    r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
    true
)]
#[case::unauthorized_grant(401, r#"{"error":"invalid_grant"}"#, true)]
#[case::bad_client(401, r#"{"error":"invalid_client"}"#, false)]
#[case::server_error(500, "invalid_grant", false)]
#[case::other_bad_request(400, r#"{"error":"invalid_request"}"#, false)]
fn refused_grants_are_recognised(#[case] status: u16, #[case] body: &str, #[case] expected: bool) {
    assert_eq!(refused_grant(status, body), expected);
}

#[tokio::test]
async fn refused_refresh_is_a_sign_in_expired_error() {
    let (url, _) = token_stub(vec![REFUSED]).await;
    assert!(matches!(
        source(&url).access_token().await,
        Err(Error::SignInExpired)
    ));
}

#[tokio::test]
async fn refused_source_stops_asking_google() {
    let (url, hits) = token_stub(vec![REFUSED]).await;
    let ts = source(&url);
    for _ in 0..5 {
        let _ = ts.access_token().await;
    }
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn refusal_hook_runs_once() {
    let (url, _) = token_stub(vec![REFUSED]).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let ts = source(&url).with_on_refused(Box::new(move || {
        c.fetch_add(1, Ordering::SeqCst);
    }));
    let _ = ts.access_token().await;
    let _ = ts.probe().await;
    let _ = ts.access_token().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn persisted_refusal_asks_google_nothing() {
    let (url, hits) = token_stub(vec![GRANTED]).await;
    let ts = source(&url);
    ts.mark_refused();
    let _ = ts.access_token().await;
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn accepted_probe_clears_the_refusal() {
    let (url, _) = token_stub(vec![REFUSED, GRANTED]).await;
    let ts = source(&url);
    let _ = ts.access_token().await;
    ts.probe().await.unwrap();
    assert!(!ts.refused());
}

#[tokio::test]
async fn other_token_errors_keep_their_status() {
    let (url, _) = token_stub(vec![(500, "oops")]).await;
    assert!(matches!(
        source(&url).access_token().await,
        Err(Error::Upstream { status: 500, .. })
    ));
}

#[tokio::test]
async fn other_token_errors_do_not_mark_refused() {
    let (url, _) = token_stub(vec![(500, "oops")]).await;
    let ts = source(&url);
    let _ = ts.access_token().await;
    assert!(!ts.refused());
}
