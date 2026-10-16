//! Mobile-store behavior and C ABI ownership exercised against real SQLite files.

use super::*;
use rstest::{fixture, rstest};
use tempfile::TempDir;

#[fixture]
fn home() -> TempDir {
    tempfile::tempdir().unwrap()
}

#[fixture]
fn engine(home: TempDir) -> (TempDir, MobileEngine) {
    let engine = MobileEngine::open(home.path()).unwrap();
    (home, engine)
}

#[rstest]
fn demo_mail_is_available(engine: (TempDir, MobileEngine)) {
    let result = engine
        .1
        .dispatch(r#"{"method":"threads.list","params":{}}"#)
        .unwrap();
    assert!(result["total"].as_i64().unwrap() > 0);
}

#[rstest]
fn opening_again_keeps_one_demo_account(home: TempDir) {
    drop(MobileEngine::open(home.path()).unwrap());
    let engine = MobileEngine::open(home.path()).unwrap();
    assert_eq!(engine.store.accounts().unwrap().len(), 1);
}

#[rstest]
fn a_thread_contains_real_message_bodies(engine: (TempDir, MobileEngine)) {
    let thread = engine
        .1
        .store
        .threads(&ThreadFilter {
            limit: 1,
            ..Default::default()
        })
        .unwrap()
        .0
        .remove(0);
    let result = engine
        .1
        .dispatch(&json!({"method":"threads.get","params":{"id":thread.id}}).to_string())
        .unwrap();
    assert!(
        !result["messages"][0]["body_text"]
            .as_str()
            .unwrap()
            .is_empty()
    );
}

#[rstest]
fn status_survives_reopening(engine: (TempDir, MobileEngine)) {
    let (home, engine) = engine;
    let id = engine
        .store
        .threads(&ThreadFilter {
            limit: 1,
            ..Default::default()
        })
        .unwrap()
        .0
        .remove(0)
        .id;
    engine
        .dispatch(
            &json!({"method":"threads.update","params":{"id":id,"status":"archive"}}).to_string(),
        )
        .unwrap();
    drop(engine);
    assert_eq!(
        MobileEngine::open(home.path())
            .unwrap()
            .store
            .thread(&id)
            .unwrap()
            .status,
        "archive"
    );
}

#[rstest]
fn search_reads_the_local_index(engine: (TempDir, MobileEngine)) {
    let result = engine
        .1
        .dispatch(r#"{"method":"search","params":{"query":"Maya"}}"#)
        .unwrap();
    assert!(!result["threads"].as_array().unwrap().is_empty());
}

#[rstest]
fn statuses_accept_the_mobile_request_shape(engine: (TempDir, MobileEngine)) {
    let result = engine
        .1
        .dispatch(r#"{"method":"statuses.list","params":{}}"#)
        .unwrap();
    assert!(
        result
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["key"] == "archive")
    );
}

#[rstest]
fn an_unknown_status_cannot_change_a_thread(engine: (TempDir, MobileEngine)) {
    let id = engine
        .1
        .store
        .threads(&ThreadFilter {
            limit: 1,
            ..Default::default()
        })
        .unwrap()
        .0
        .remove(0)
        .id;
    assert!(
        engine
            .1
            .dispatch(
                &json!({"method":"threads.update","params":{"id":id,"status":"invalid"}})
                    .to_string()
            )
            .is_err()
    );
}

#[rstest]
#[case::unknown_method(r#"{"method":"send"}"#)]
#[case::malformed_json("{")]
#[case::missing_id(r#"{"method":"threads.get","params":{}}"#)]
#[case::missing_thread(r#"{"method":"threads.get","params":{"id":"missing"}}"#)]
#[case::invalid_status(
    r#"{"method":"threads.update","params":{"id":"missing","status":"invalid"}}"#
)]
fn invalid_requests_return_errors(engine: (TempDir, MobileEngine), #[case] request: &str) {
    assert!(engine.1.dispatch(request).is_err());
}

#[rstest]
fn the_c_bridge_opens_queries_and_releases(home: TempDir) {
    let path = CString::new(home.path().to_str().unwrap()).unwrap();
    let request = CString::new(r#"{"method":"threads.list","params":{}}"#).unwrap();
    let mut error = std::ptr::null_mut();
    let result = unsafe {
        let engine = mermoz_mobile_open(path.as_ptr(), &mut error);
        let response = mermoz_mobile_request(engine, request.as_ptr());
        let result: Value = serde_json::from_slice(CStr::from_ptr(response).to_bytes()).unwrap();
        mermoz_mobile_string_free(response);
        mermoz_mobile_string_free(error);
        mermoz_mobile_close(engine);
        result
    };
    assert!(result["result"]["total"].as_i64().unwrap() > 0);
}

#[test]
fn the_c_bridge_rejects_a_null_engine() {
    let value = unsafe { mermoz_mobile_request(std::ptr::null_mut(), std::ptr::null()) };
    let result: Value =
        unsafe { serde_json::from_slice(CStr::from_ptr(value).to_bytes()).unwrap() };
    unsafe { mermoz_mobile_string_free(value) };
    assert_eq!(result["error"], "Missing engine or request");
}
