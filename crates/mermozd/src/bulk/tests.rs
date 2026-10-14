//! How a run's snapshot splits into writes.

use super::*;
use rstest::rstest;

fn t(id: &str, siblings: &[&str]) -> (String, Vec<String>) {
    (id.into(), siblings.iter().map(|s| s.to_string()).collect())
}

/// Three targets: `a` and `b` share a conversation with `s`, `c` stands alone.
fn step() -> Vec<(String, Vec<String>)> {
    vec![t("a", &["s", "b"]), t("b", &["s", "a"]), t("c", &[])]
}

#[rstest]
fn a_write_takes_its_targets_and_their_siblings_once() {
    assert_eq!(writes(&step(), 2)[0].1, vec!["a", "s", "b"]);
}

#[rstest]
fn a_thread_already_written_is_not_written_again() {
    assert_eq!(writes(&step(), 1)[1].1, Vec::<String>::new());
}

#[rstest]
fn a_write_counts_only_its_targets() {
    assert_eq!(writes(&step(), 2)[0].0, vec!["a", "b"]);
}

#[rstest]
fn the_last_write_takes_the_rest() {
    assert_eq!(writes(&step(), 2)[1].1, vec!["c"]);
}
