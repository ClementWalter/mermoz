//! Which label change each thread asks of Gmail, and how threads pack into calls.

use super::*;
use rstest::rstest;

fn set(labels: &[&str]) -> BTreeSet<String> {
    labels.iter().map(|l| l.to_string()).collect()
}

fn delta(add: &[&str], remove: &[&str]) -> Option<Delta> {
    Some((set(add), set(remove)))
}

#[rstest]
#[case::delete_goes_to_trash(true, false, &[], &[], delta(&["TRASH"], &["INBOX"]))]
#[case::delete_already_in_trash(true, true, &[], &[], None)]
#[case::undo_back_to_inbox(false, true, &["INBOX"], &[], delta(&["INBOX"], &["TRASH"]))]
#[case::undo_to_archive(false, true, &[], &[], delta(&[], &["TRASH"]))]
#[case::archive(false, false, &[], &["INBOX"], delta(&[], &["INBOX"]))]
#[case::nothing_to_do(false, false, &["INBOX"], &["INBOX"], None)]
fn a_thread_asks_gmail_for(
    #[case] deleted: bool,
    #[case] trashed: bool,
    #[case] desired: &[&str],
    #[case] current: &[&str],
    #[case] expected: Option<Delta>,
) {
    assert_eq!(
        label_delta(deleted, trashed, &set(desired), &set(current)),
        expected
    );
}

#[rstest]
fn every_delete_shares_one_change() {
    let empty = BTreeSet::new();
    assert_eq!(
        label_delta(true, false, &empty, &empty),
        label_delta(true, false, &set(&["INBOX"]), &set(&["Mermoz/Todo"]))
    );
}

fn trash() -> Delta {
    (set(&["TRASH"]), set(&["INBOX"]))
}

fn archive() -> Delta {
    (set(&[]), set(&["INBOX"]))
}

/// The user's delete, then a bulk run's deletes and archives, in queue order.
fn queue() -> Vec<(bool, Delta, &'static str)> {
    vec![
        (false, trash(), "mine"),
        (true, trash(), "run-1"),
        (true, archive(), "run-2"),
        (true, trash(), "run-3"),
    ]
}

#[rstest]
fn ops_group_by_who_queued_them_and_their_change() {
    let groups: Vec<Vec<&str>> = group(queue()).into_iter().map(|(_, ops)| ops).collect();
    assert_eq!(
        groups,
        vec![vec!["mine"], vec!["run-1", "run-3"], vec!["run-2"]]
    );
}

#[rstest]
fn a_group_keeps_its_change() {
    assert_eq!(group(queue())[1].0, trash());
}

#[rstest]
#[case::one_call_when_it_fits(&[400, 600], 1000, vec![vec![0, 1]])]
#[case::a_thread_never_splits(&[600, 600], 1000, vec![vec![0], vec![1]])]
#[case::queue_order_kept(&[1, 1, 1], 2, vec![vec![0, 1], vec![2]])]
#[case::fifteen_hundred_single_message_threads(&[1; 1500], 1000, vec![(0..1000).collect(), (1000..1500).collect()])]
fn threads_pack_into_calls(
    #[case] sizes: &[usize],
    #[case] max: usize,
    #[case] calls: Vec<Vec<usize>>,
) {
    assert_eq!(pack(sizes, max).0, calls);
}

#[rstest]
#[case::no_stored_message(&[3, 0], vec![1])]
#[case::over_the_limit(&[1001, 1], vec![0])]
fn a_thread_that_fits_no_call_goes_alone(#[case] sizes: &[usize], #[case] alone: Vec<usize>) {
    assert_eq!(pack(sizes, 1000).1, alone);
}

#[rstest]
#[case::sign_in_refused(Error::SignInExpired, true)]
#[case::label_refused(Error::Upstream { status: 400, body: "bad label".into() }, false)]
#[case::thread_gone(Error::Upstream { status: 404, body: String::new() }, false)]
#[case::other(Error::other("odd"), false)]
fn an_error_halts_the_round(#[case] e: Error, #[case] halts: bool) {
    assert_eq!(halts_round(&e), halts);
}
