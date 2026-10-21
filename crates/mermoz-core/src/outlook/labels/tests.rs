use super::*;
use crate::outlook::Flag;
use rstest::{fixture, rstest};

fn folder(id: &str, name: &str, parent: &str) -> Folder {
    Folder {
        id: id.into(),
        display_name: name.into(),
        parent_folder_id: Some(parent.into()),
        child_folder_count: 0,
        total_item_count: 10,
    }
}

/// A mailbox: the well-known folders, "Clients" with "Acme" inside, "Bills" under the
/// Inbox, "Old" under Deleted Items and "Logs" under Sync Issues.
#[fixture]
fn map() -> FolderMap {
    let all = vec![
        folder("fi", "Inbox", "root"),
        folder("fs", "Sent Items", "root"),
        folder("fd", "Deleted Items", "root"),
        folder("fj", "Junk Email", "root"),
        folder("fdr", "Drafts", "root"),
        folder("fa", "Archive", "root"),
        folder("fo", "Outbox", "root"),
        folder("fsi", "Sync Issues", "root"),
        folder("fc", "Clients", "root"),
        folder("facme", "Acme", "fc"),
        folder("fib", "Bills", "fi"),
        folder("fdx", "Old", "fd"),
        folder("flog", "Logs", "fsi"),
    ];
    let roles = [
        ("inbox", "fi"),
        ("sentitems", "fs"),
        ("deleteditems", "fd"),
        ("junkemail", "fj"),
        ("drafts", "fdr"),
        ("archive", "fa"),
        ("outbox", "fo"),
        ("syncissues", "fsi"),
    ]
    .iter()
    .map(|(n, id)| (n.to_string(), id.to_string()))
    .collect();
    FolderMap::build(roles, &all)
}

fn set(labels: &[&str]) -> BTreeSet<String> {
    labels.iter().map(|l| l.to_string()).collect()
}

fn meta(folder: &str, read: bool, categories: &[&str], flagged: bool) -> MessageMeta {
    MessageMeta {
        id: "m".into(),
        conversation_id: Some("c".into()),
        parent_folder_id: Some(folder.into()),
        is_read: Some(read),
        categories: categories.iter().map(|c| c.to_string()).collect(),
        flag: Some(Flag {
            flag_status: if flagged { "flagged" } else { "notFlagged" }.into(),
        }),
        received_date_time: None,
        removed: None,
    }
}

#[rstest]
#[case::inbox("fi", Place::Inbox)]
#[case::sent("fs", Place::Sent)]
#[case::deleted("fd", Place::Trash)]
#[case::junk("fj", Place::Spam)]
#[case::drafts("fdr", Place::Drafts)]
#[case::archive("fa", Place::Archive)]
#[case::outbox("fo", Place::Skip)]
#[case::folder_in_deleted_items("fdx", Place::Trash)]
#[case::folder_in_sync_issues("flog", Place::Skip)]
#[case::user_folder("fc", Place::Folder { label: "folder:fc".into(), name: "Clients".into() })]
#[case::nested_folder("facme", Place::Folder { label: "folder:facme".into(), name: "Clients/Acme".into() })]
#[case::folder_in_inbox("fib", Place::Folder { label: "folder:fib".into(), name: "Bills".into() })]
fn folders_have_places(map: FolderMap, #[case] id: &str, #[case] want: Place) {
    assert_eq!(map.place(id), Some(&want));
}

#[rstest]
fn imported_folders_start_with_the_inbox(map: FolderMap) {
    assert_eq!(map.synced(), vec!["fi", "fs", "fa", "facme", "fc", "fib"]);
}

#[rstest]
fn total_counts_imported_folders_only(map: FolderMap) {
    assert_eq!(map.total(), 60);
}

#[rstest]
#[case::unread_inbox(meta("fi", false, &[], false), &["INBOX", "UNREAD"])]
#[case::read_sent(meta("fs", true, &[], false), &["SENT"])]
#[case::archived(meta("fa", true, &[], false), &[])]
#[case::deleted(meta("fd", true, &[], false), &["TRASH"])]
#[case::junk(meta("fj", true, &[], false), &["SPAM"])]
#[case::drafts(meta("fdr", true, &[], false), &["DRAFT"])]
#[case::user_folder(meta("fc", true, &[], false), &["folder:fc"])]
#[case::categories(meta("fi", true, &["Travel", "Mermoz/To do"], false), &["INBOX", "cat:Mermoz/To do", "cat:Travel"])]
#[case::flagged(meta("fi", true, &[], true), &["INBOX", "STARRED"])]
#[case::unknown_folder(meta("nowhere", true, &[], false), &[])]
fn messages_get_labels(map: FolderMap, #[case] m: MessageMeta, #[case] want: &[&str]) {
    assert_eq!(labels_for(&m, &map), set(want));
}

#[rstest]
fn folder_labels_are_named_by_path(map: FolderMap) {
    assert!(
        map.folder_labels()
            .contains(&("folder:facme".to_string(), "Clients/Acme".to_string()))
    );
}

/// One message with these labels, edited with `add` and `remove`.
fn edit(map: &FolderMap, labels: &[&str], add: &[&str], remove: &[&str]) -> Option<Edit> {
    plan_thread(&[("m1".into(), set(labels))], &set(add), &set(remove), map)
        .into_iter()
        .next()
}

#[rstest]
#[case::archive(&["INBOX"], &[], &["INBOX"], Some("fa"))]
#[case::archive_into_project_folder(&["INBOX"], &["Clients"], &["INBOX"], Some("fc"))]
#[case::back_to_inbox(&[], &["INBOX"], &[], Some("fi"))]
#[case::folder_back_to_inbox(&["folder:fc"], &["INBOX"], &[], Some("fi"))]
#[case::trash(&["INBOX"], &["TRASH"], &["INBOX"], Some("fd"))]
#[case::untrash_to_inbox(&["TRASH"], &["INBOX"], &["TRASH"], Some("fi"))]
#[case::untrash_to_archive(&["TRASH"], &[], &["TRASH"], Some("fa"))]
#[case::file_archived_mail(&[], &["Clients"], &[], Some("fc"))]
#[case::unfile(&["folder:fc"], &[], &["Clients"], Some("fa"))]
#[case::project_on_inbox_mail_stays(&["INBOX"], &["Clients"], &[], None)]
#[case::sent_mail_stays_out_of_the_inbox(&["SENT"], &[], &["INBOX"], None)]
fn moves(
    map: FolderMap,
    #[case] labels: &[&str],
    #[case] add: &[&str],
    #[case] remove: &[&str],
    #[case] want: Option<&str>,
) {
    assert_eq!(
        edit(&map, labels, add, remove).and_then(|e| e.move_to),
        want.map(String::from)
    );
}

#[rstest]
#[case::mark_read(&["INBOX", "UNREAD"], &[], &["UNREAD"], json!({ "isRead": true }))]
#[case::mark_unread(&["INBOX"], &["UNREAD"], &[], json!({ "isRead": false }))]
#[case::status_category(&["INBOX"], &["Mermoz/To do"], &[], json!({ "categories": ["Mermoz/To do"] }))]
#[case::status_change(&["INBOX", "cat:Mermoz/To do"], &["Mermoz/Waiting"], &["Mermoz/To do"], json!({ "categories": ["Mermoz/Waiting"] }))]
#[case::project_on_inbox_mail(&["INBOX", "cat:Travel"], &["Clients"], &[], json!({ "categories": ["Clients", "Travel"] }))]
#[case::star(&["INBOX"], &["STARRED"], &[], json!({ "flag": { "flagStatus": "flagged" } }))]
fn patches(
    map: FolderMap,
    #[case] labels: &[&str],
    #[case] add: &[&str],
    #[case] remove: &[&str],
    #[case] want: Value,
) {
    assert_eq!(
        edit(&map, labels, add, remove).and_then(|e| e.patch),
        Some(want)
    );
}

#[rstest]
fn filing_into_a_folder_adds_no_category_of_its_name(map: FolderMap) {
    assert_eq!(edit(&map, &[], &["Clients"], &[]).unwrap().patch, None);
}

#[rstest]
#[case::archived(&["INBOX", "UNREAD"], &[], &["INBOX", "UNREAD"], &[])]
#[case::filed(&["INBOX"], &["Clients"], &["INBOX"], &["folder:fc"])]
#[case::trashed(&["INBOX", "cat:Travel"], &["TRASH"], &["INBOX"], &["TRASH", "cat:Travel"])]
#[case::labelled(&["INBOX"], &["Travel"], &[], &["INBOX", "cat:Travel"])]
fn labels_after_an_edit(
    map: FolderMap,
    #[case] labels: &[&str],
    #[case] add: &[&str],
    #[case] remove: &[&str],
    #[case] want: &[&str],
) {
    assert_eq!(
        edit(&map, labels, add, remove).unwrap().labels_after,
        set(want)
    );
}

#[rstest]
fn nothing_to_do_is_no_edit(map: FolderMap) {
    assert_eq!(edit(&map, &["INBOX"], &[], &["Travel"]), None);
}

#[rstest]
fn a_thread_of_sent_mail_alone_goes_back_to_the_inbox(map: FolderMap) {
    let edits = plan_thread(
        &[("m1".into(), set(&["SENT"]))],
        &set(&["INBOX"]),
        &set(&[]),
        &map,
    );
    assert_eq!(edits[0].move_to.as_deref(), Some("fi"));
}

#[rstest]
fn unarchiving_moves_only_received_mail(map: FolderMap) {
    let edits = plan_thread(
        &[("m1".into(), set(&[])), ("m2".into(), set(&["SENT"]))],
        &set(&["INBOX"]),
        &set(&[]),
        &map,
    );
    assert_eq!(
        edits.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        vec!["m1"]
    );
}

#[rstest]
#[case::whole_seconds(Some("7"), Some(std::time::Duration::from_secs(7)))]
#[case::capped(Some("100000"), Some(std::time::Duration::from_secs(300)))]
#[case::http_date(Some("Wed, 21 Oct 2026 07:28:00 GMT"), None)]
#[case::absent(None, None)]
fn retry_after_is_read_in_seconds(
    #[case] header: Option<&str>,
    #[case] want: Option<std::time::Duration>,
) {
    assert_eq!(super::super::retry_after(header), want);
}
