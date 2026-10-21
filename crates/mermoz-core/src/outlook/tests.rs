//! The Graph client against the in-process stub Graph.

use super::stub::{Graph, Shared};
use super::*;
use crate::oauth::ClientConfig;
use rstest::{fixture, rstest};

const MIME: &str = "From: Ana <ana@example.com>\r\nTo: me@example.com\r\nSubject: Hello\r\nMessage-ID: <a1@example.com>\r\n\r\nHi there\r\n";

struct Env {
    graph: Shared,
    client: Outlook,
}

/// A stub mailbox with three inbox messages and a Graph client signed in to it.
#[fixture]
fn env() -> Env {
    let graph: Shared = Arc::new(std::sync::Mutex::new(Graph::new("ana@outlook.example")));
    {
        let mut g = graph.lock().unwrap();
        g.add_message("m1", "c1", "f-inbox", false, MIME, 3_000);
        g.add_message("m2", "c2", "f-inbox", true, MIME, 2_000);
        g.add_message("m3", "c3", "f-inbox", true, MIME, 1_000);
    }
    let port = super::stub::start(graph.clone());
    let mut cfg = ClientConfig::microsoft("ms-cid".into());
    cfg.token_url = format!("http://127.0.0.1:{port}/ms/token");
    let client = Outlook::new(
        format!("http://127.0.0.1:{port}/graph"),
        Arc::new(TokenSource::new(cfg, "rt".into())),
    );
    Env { graph, client }
}

/// Walks a folder's delta to the end; returns the entries and the delta link.
async fn walk(c: &Outlook, folder: &str, link: Option<String>) -> (Vec<MessageMeta>, String) {
    let mut link = link;
    let mut out = vec![];
    loop {
        let page = c.delta(folder, link.as_deref()).await.unwrap().unwrap();
        out.extend(page.value);
        if let Some(d) = page.delta_link {
            return (out, d);
        }
        link = page.next_link;
    }
}

fn ids(v: &[MessageMeta]) -> Vec<&str> {
    v.iter().map(|m| m.id.as_str()).collect()
}

#[rstest]
#[tokio::test]
async fn personal_accounts_are_known_by_their_sign_in_name(env: Env) {
    assert_eq!(
        env.client.me().await.unwrap().email().as_deref(),
        Some("ana@outlook.example")
    );
}

#[rstest]
#[tokio::test]
async fn every_request_asks_for_immutable_ids(env: Env) {
    env.client.me().await.unwrap();
    env.client.mime("m1").await.unwrap();
    assert!(
        env.graph
            .lock()
            .unwrap()
            .prefer
            .iter()
            .all(|p| p.contains("IdType=\"ImmutableId\""))
    );
}

#[rstest]
#[tokio::test]
async fn calls_carry_the_refreshed_access_token(env: Env) {
    env.client.me().await.unwrap();
    assert_eq!(
        env.graph.lock().unwrap().bearers,
        ["graph-access-1".to_string()].into()
    );
}

#[rstest]
#[tokio::test]
async fn a_first_delta_walks_every_page(env: Env) {
    let (items, _) = walk(&env.client, "f-inbox", None).await;
    assert_eq!(ids(&items), vec!["m1", "m2", "m3"]);
}

#[rstest]
#[tokio::test]
async fn a_delta_link_brings_only_what_changed(env: Env) {
    let (_, link) = walk(&env.client, "f-inbox", None).await;
    env.graph.lock().unwrap().edit("m2", |m| m.is_read = false);
    let (items, _) = walk(&env.client, "f-inbox", Some(link)).await;
    assert_eq!(items[0].is_read, Some(false));
}

#[rstest]
#[tokio::test]
async fn a_message_moved_out_is_reported_removed(env: Env) {
    let (_, link) = walk(&env.client, "f-inbox", None).await;
    env.graph.lock().unwrap().move_message("m3", "f-archive");
    let (items, _) = walk(&env.client, "f-inbox", Some(link)).await;
    assert!(items[0].removed.is_some());
}

#[rstest]
#[tokio::test]
async fn an_expired_delta_link_asks_for_a_new_walk(env: Env) {
    let (_, link) = walk(&env.client, "f-inbox", None).await;
    env.graph.lock().unwrap().expire_deltas = true;
    assert!(
        env.client
            .delta("f-inbox", Some(&link))
            .await
            .unwrap()
            .is_none()
    );
}

#[rstest]
#[tokio::test]
async fn throttled_calls_are_retried_after_the_wait_asked(env: Env) {
    env.graph.lock().unwrap().throttle = 2;
    assert!(env.client.me().await.is_ok());
}

#[rstest]
#[tokio::test]
async fn throttling_is_seen_before_success(env: Env) {
    env.graph.lock().unwrap().throttle = 2;
    env.client.me().await.unwrap();
    assert_eq!(env.graph.lock().unwrap().throttled, 2);
}

#[rstest]
#[tokio::test]
async fn the_listing_is_newest_first(env: Env) {
    let page = env.client.messages_page(None).await.unwrap();
    assert_eq!(page.value[0].id, "m1");
}

#[rstest]
#[tokio::test]
async fn the_mime_source_is_fetched_as_is(env: Env) {
    assert_eq!(env.client.mime("m1").await.unwrap(), MIME.as_bytes());
}

#[rstest]
#[tokio::test]
async fn a_send_posts_the_mime_message(env: Env) {
    let raw = "From: me@example.com\r\nTo: ana@example.com\r\nSubject: Re: Hello\r\nIn-Reply-To: <a1@example.com>\r\n\r\nThanks\r\n";
    env.client.send_mime(raw.as_bytes()).await.unwrap();
    assert_eq!(env.graph.lock().unwrap().sent, vec![raw.to_string()]);
}

#[rstest]
#[tokio::test]
async fn a_reply_lands_in_the_conversation_it_answers(env: Env) {
    let raw = "From: me@example.com\r\nTo: ana@example.com\r\nSubject: Re: Hello\r\nIn-Reply-To: <a1@example.com>\r\n\r\nThanks\r\n";
    env.client.send_mime(raw.as_bytes()).await.unwrap();
    let g = env.graph.lock().unwrap();
    let sent = g.messages.values().find(|m| m.folder == "f-sent").unwrap();
    assert_eq!(sent.conversation_id, "c1");
}

#[rstest]
#[tokio::test]
async fn a_moved_message_keeps_its_id(env: Env) {
    env.client.move_to("m1", "f-archive").await.unwrap();
    assert_eq!(
        env.client
            .message("m1")
            .await
            .unwrap()
            .parent_folder_id
            .as_deref(),
        Some("f-archive")
    );
}

#[rstest]
#[tokio::test]
async fn a_patch_marks_read(env: Env) {
    env.client
        .update("m1", &json!({ "isRead": true }))
        .await
        .unwrap();
    assert_eq!(env.client.message("m1").await.unwrap().is_read, Some(true));
}

#[rstest]
#[tokio::test]
async fn a_missing_well_known_folder_is_none(env: Env) {
    assert_eq!(env.client.well_known("scheduled").await.unwrap(), None);
}

#[rstest]
#[tokio::test]
async fn child_folders_are_listed(env: Env) {
    env.graph
        .lock()
        .unwrap()
        .add_folder("f-bills", "Bills", "f-inbox");
    assert_eq!(
        env.client.folders(Some("f-inbox")).await.unwrap()[0].display_name,
        "Bills"
    );
}

#[rstest]
#[case::with_offset("2026-10-21T07:28:00+02:00", 1_792_560_480_000)]
#[case::utc("2026-10-21T05:28:00Z", 1_792_560_480_000)]
#[case::garbage("yesterday", 0)]
fn received_time_is_read_as_milliseconds(#[case] when: &str, #[case] ms: i64) {
    let m = MessageMeta {
        received_date_time: Some(when.into()),
        ..Default::default()
    };
    assert_eq!(m.received_ms(), ms);
}
