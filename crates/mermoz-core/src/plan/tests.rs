//! Plan checking: the schema is strict, names are made canonical, and nothing that
//! would send mail gets through.

use super::*;
use rstest::{fixture, rstest};

#[fixture]
fn vocab() -> Vocab {
    Vocab {
        statuses: [
            ("unread", "Unread"),
            ("todo", "To do"),
            ("waiting", "Waiting"),
            ("review", "Review"),
            ("done", "Done"),
            ("archive", "Archive"),
            ("deleted", "Deleted"),
        ]
        .iter()
        .map(|(k, n)| (k.to_string(), n.to_string()))
        .collect(),
        projects: vec!["Travel".into(), "Clients/Acme".into()],
        accounts: vec![("acc_1".into(), "me@example.com".into())],
    }
}

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 13).unwrap()
}

/// One step with this action and an empty filter, as the AI writes it.
fn with_action(action: Value) -> Value {
    json!({ "understood": true, "question": null, "steps": [{ "filter": {}, "action": action }] })
}

fn with_filter(filter: Value) -> Value {
    json!({ "understood": true, "question": null,
            "steps": [{ "filter": filter, "action": { "kind": "set_status", "status": "archive" } }] })
}

fn checked(v: Value, vocab: &Vocab) -> Result<Outcome, String> {
    validate(&v, vocab, today())
}

fn first_step(v: Value, vocab: &Vocab) -> Step {
    match checked(v, vocab).unwrap() {
        Outcome::Plan(p) => p.steps[0].clone(),
        other => panic!("not a plan: {other:?}"),
    }
}

/// The user's example, as the AI fills the strict schema (every key, nulls included).
fn archive_old_not_done() -> Value {
    json!({ "understood": true, "question": null, "steps": [{
        "filter": { "status_in": [], "status_not_in": ["done"], "older_than": { "amount": 5, "unit": "month" },
                    "newer_than": null, "from": [], "projects": [], "not_projects": [], "unread": null,
                    "has_attachment": null, "account": null, "query": null },
        "action": { "kind": "set_status", "status": "archive", "project": null, "until": null } }] })
}

#[rstest]
fn the_full_schema_form_is_a_plan(vocab: Vocab) {
    assert_eq!(
        first_step(archive_old_not_done(), &vocab)
            .filter
            .status_not_in,
        vec!["done".to_string()]
    );
}

#[rstest]
fn a_plan_round_trips_through_its_own_json(vocab: Vocab) {
    let step = first_step(archive_old_not_done(), &vocab);
    let again = json!({ "steps": [step.clone()] });
    assert_eq!(first_step(again, &vocab), step);
}

#[rstest]
#[case::send("send")]
#[case::send_email("send_email")]
#[case::forward("forward")]
#[case::reply("reply")]
#[case::draft_reply("draft_reply")]
fn outgoing_mail_is_refused(vocab: Vocab, #[case] kind: &str) {
    assert_eq!(
        checked(with_action(json!({ "kind": kind })), &vocab),
        Err("Mermoz never sends, replies to or forwards mail for you.".into())
    );
}

#[rstest]
#[case::unsubscribe("unsubscribe")]
#[case::create_rule("create_rule")]
#[case::archive_as_kind("archive")]
fn unknown_actions_are_refused(vocab: Vocab, #[case] kind: &str) {
    assert!(
        checked(with_action(json!({ "kind": kind })), &vocab)
            .unwrap_err()
            .contains("not something Mermoz can do")
    );
}

#[rstest]
fn an_unknown_field_is_refused(vocab: Vocab) {
    let v = with_action(json!({ "kind": "mark_read", "to": "nora@example.com" }));
    assert!(checked(v, &vocab).unwrap_err().contains("doesn't know"));
}

#[rstest]
fn an_unknown_top_level_field_is_refused(vocab: Vocab) {
    let mut v = archive_old_not_done();
    v["execute"] = json!(true);
    assert!(checked(v, &vocab).is_err());
}

#[rstest]
#[case::by_name("Done", "done")]
#[case::by_key("todo", "todo")]
#[case::spaced_name("to do", "todo")]
fn statuses_are_made_keys(vocab: Vocab, #[case] given: &str, #[case] want: &str) {
    let step = first_step(
        with_action(json!({ "kind": "set_status", "status": given })),
        &vocab,
    );
    assert_eq!(
        step.action,
        Action::SetStatus {
            status: want.into()
        }
    );
}

#[rstest]
fn an_unknown_status_is_refused(vocab: Vocab) {
    let v = with_action(json!({ "kind": "set_status", "status": "someday" }));
    assert_eq!(
        checked(v, &vocab),
        Err("There is no status called 'someday'.".into())
    );
}

#[rstest]
#[case::unread("unread")]
#[case::review("review")]
fn some_statuses_cannot_be_targets(vocab: Vocab, #[case] status: &str) {
    let v = with_action(json!({ "kind": "set_status", "status": status }));
    assert!(
        checked(v, &vocab)
            .unwrap_err()
            .starts_with("Mermoz can't move mail")
    );
}

#[rstest]
fn moving_to_deleted_is_a_delete(vocab: Vocab) {
    let step = first_step(
        with_action(json!({ "kind": "set_status", "status": "deleted" })),
        &vocab,
    );
    assert_eq!(step.action, Action::Delete);
}

#[rstest]
fn a_filter_project_is_made_canonical(vocab: Vocab) {
    let step = first_step(with_filter(json!({ "projects": ["travel"] })), &vocab);
    assert_eq!(step.filter.projects, vec!["Travel".to_string()]);
}

#[rstest]
fn an_unknown_filter_project_is_refused(vocab: Vocab) {
    let v = with_filter(json!({ "projects": ["Holidays"] }));
    assert_eq!(
        checked(v, &vocab),
        Err("There is no project called 'Holidays'.".into())
    );
}

#[rstest]
fn adding_to_a_new_project_is_allowed(vocab: Vocab) {
    let step = first_step(
        with_action(json!({ "kind": "add_project", "project": "Holidays" })),
        &vocab,
    );
    assert_eq!(
        step.action,
        Action::AddProject {
            project: "Holidays".into()
        }
    );
}

#[rstest]
fn removing_from_an_unknown_project_is_refused(vocab: Vocab) {
    let v = with_action(json!({ "kind": "remove_project", "project": "Holidays" }));
    assert!(checked(v, &vocab).is_err());
}

#[rstest]
fn a_project_action_needs_a_project(vocab: Vocab) {
    let v = with_action(json!({ "kind": "add_project", "project": null }));
    assert_eq!(
        checked(v, &vocab),
        Err("'add_project' needs a project.".into())
    );
}

#[rstest]
#[case::past("2026-10-01")]
#[case::today("2026-10-13")]
#[case::not_a_date("next friday")]
fn later_needs_a_day_after_today(vocab: Vocab, #[case] until: &str) {
    let v = with_action(json!({ "kind": "later", "until": until }));
    assert!(checked(v, &vocab).is_err());
}

#[rstest]
fn later_keeps_its_day(vocab: Vocab) {
    let step = first_step(
        with_action(json!({ "kind": "later", "until": "2026-10-20" })),
        &vocab,
    );
    assert_eq!(
        step.action,
        Action::Later {
            until: "2026-10-20".into()
        }
    );
}

#[rstest]
#[case::zero(json!({ "amount": 0, "unit": "month" }))]
#[case::negative(json!({ "amount": -3, "unit": "day" }))]
#[case::unknown_unit(json!({ "amount": 3, "unit": "fortnight" }))]
fn bad_ages_are_refused(vocab: Vocab, #[case] age: Value) {
    assert!(checked(with_filter(json!({ "older_than": age })), &vocab).is_err());
}

#[rstest]
fn plural_units_are_read(vocab: Vocab) {
    let step = first_step(
        with_filter(json!({ "older_than": { "amount": 2, "unit": "years" } })),
        &vocab,
    );
    assert_eq!(
        step.filter.older_than,
        Some(Age {
            amount: 2,
            unit: Unit::Year
        })
    );
}

#[rstest]
fn an_account_by_email_becomes_its_id(vocab: Vocab) {
    let step = first_step(with_filter(json!({ "account": "ME@example.com" })), &vocab);
    assert_eq!(step.filter.account.as_deref(), Some("acc_1"));
}

#[rstest]
fn an_unknown_account_is_refused(vocab: Vocab) {
    assert!(
        checked(
            with_filter(json!({ "account": "other@example.com" })),
            &vocab
        )
        .is_err()
    );
}

#[rstest]
fn senders_are_lowercased(vocab: Vocab) {
    let step = first_step(
        with_filter(json!({ "from": [" Nora@Example.com "] })),
        &vocab,
    );
    assert_eq!(step.filter.from, vec!["nora@example.com".to_string()]);
}

#[rstest]
fn not_understood_is_a_question(vocab: Vocab) {
    let v = json!({ "understood": false, "question": "I can't send email.", "steps": [] });
    assert_eq!(
        checked(v, &vocab),
        Ok(Outcome::Question("I can't send email.".into()))
    );
}

#[rstest]
fn no_steps_is_a_question(vocab: Vocab) {
    let v = json!({ "understood": true, "question": null, "steps": [] });
    assert!(matches!(checked(v, &vocab), Ok(Outcome::Question(_))));
}

#[rstest]
fn too_many_steps_are_refused(vocab: Vocab) {
    let step = json!({ "filter": {}, "action": { "kind": "mark_read" } });
    let v = json!({ "understood": true, "steps": vec![step; MAX_STEPS + 1] });
    assert!(checked(v, &vocab).is_err());
}

#[rstest]
fn a_plan_with_a_delete_says_so(vocab: Vocab) {
    let Ok(Outcome::Plan(p)) = checked(with_action(json!({ "kind": "delete" })), &vocab) else {
        panic!("not a plan")
    };
    assert!(p.deletes());
}

#[rstest]
fn schema_lists_only_safe_actions() {
    assert_eq!(
        schema()["properties"]["steps"]["items"]["properties"]["action"]["properties"]["kind"]["enum"],
        json!([
            "set_status",
            "add_project",
            "remove_project",
            "mark_read",
            "mark_unread",
            "later",
            "delete"
        ])
    );
}

#[rstest]
fn prompt_names_the_projects(vocab: Vocab) {
    assert!(system_prompt(&vocab, today()).contains("Travel, Clients/Acme"));
}

#[rstest]
fn prompt_says_there_is_no_send(vocab: Vocab) {
    assert!(system_prompt(&vocab, today()).contains("no action to send"));
}

// ---- dates

const OCT_13: i64 = 1_791_849_600_000; // 2026-10-13T00:00:00Z

#[rstest]
#[case::months(Age { amount: 5, unit: Unit::Month }, "2026-05-13")]
#[case::days(Age { amount: 10, unit: Unit::Day }, "2026-10-03")]
#[case::weeks(Age { amount: 2, unit: Unit::Week }, "2026-09-29")]
#[case::years(Age { amount: 1, unit: Unit::Year }, "2025-10-13")]
fn cutoff_goes_back_by_calendar(#[case] age: Age, #[case] want: &str) {
    let d = DateTime::<Utc>::from_timestamp_millis(cutoff(OCT_13, age)).unwrap();
    assert_eq!(d.date_naive().to_string(), want);
}

// ---- plain words

#[rstest]
#[case(0, "0")]
#[case(999, "999")]
#[case(1842, "1,842")]
#[case(1234567, "1,234,567")]
fn counts_have_thousands_separators(#[case] n: usize, #[case] want: &str) {
    assert_eq!(count(n), want);
}

#[rstest]
fn the_users_example_reads_as_one_sentence(vocab: Vocab) {
    let step = first_step(archive_old_not_done(), &vocab);
    assert_eq!(
        sentence(&step, 1842, &vocab),
        "Archive 1,842 threads older than 5 months that aren't Done"
    );
}

#[rstest]
fn the_users_example_reads_as_conditions(vocab: Vocab) {
    let step = first_step(archive_old_not_done(), &vocab);
    assert_eq!(
        interpretation(&step.filter, &vocab),
        "status ≠ Done AND older than 5 months"
    );
}

#[rstest]
fn an_empty_filter_says_every_email(vocab: Vocab) {
    assert_eq!(
        interpretation(&Filter::default(), &vocab),
        "every email in Mermoz"
    );
}

#[rstest]
fn a_delete_says_where_mail_goes(vocab: Vocab) {
    let step = Step {
        filter: Filter {
            from: vec!["news.example".into()],
            ..Default::default()
        },
        action: Action::Delete,
    };
    assert_eq!(
        sentence(&step, 1, &vocab),
        "Delete 1 thread from news.example (to Gmail Trash)"
    );
}

#[rstest]
fn a_status_move_names_the_status(vocab: Vocab) {
    let step = Step {
        filter: Filter {
            projects: vec!["Travel".into()],
            ..Default::default()
        },
        action: Action::SetStatus {
            status: "done".into(),
        },
    };
    assert_eq!(
        sentence(&step, 3, &vocab),
        "Move 3 threads in Travel to Done"
    );
}

// ---- senders, candidates and picks

#[rstest]
#[case::description("gemini notes from google meet")]
#[case::too_many_words("the notes that gemini writes")]
#[case::about("mail about the gourmet club")]
fn a_description_in_the_sender_field_is_refused(vocab: Vocab, #[case] from: &str) {
    let v = with_filter(json!({ "from": [from] }));
    assert!(checked(v, &vocab).unwrap_err().contains("is not a sender"));
}

#[rstest]
#[case::name("Gemini")]
#[case::full_name("Pierre-Etienne Nageotte")]
#[case::list_words("esrin gourmet club")]
#[case::address("gemini@google.example")]
#[case::domain("news.example")]
fn a_name_or_address_is_a_sender(vocab: Vocab, #[case] from: &str) {
    assert!(checked(with_filter(json!({ "from": [from] })), &vocab).is_ok());
}

#[rstest]
fn a_subject_prefix_is_kept(vocab: Vocab) {
    let step = first_step(with_filter(json!({ "subject_prefix": "Notes:" })), &vocab);
    assert_eq!(step.filter.subject_prefix.as_deref(), Some("Notes:"));
}

fn gemini() -> CandidateSender {
    CandidateSender {
        email: "gemini@google.example".into(),
        name: "Gemini".into(),
        threads: 40,
        subjects: vec!["Notes:".into()],
    }
}

#[rstest]
fn the_prompt_lists_a_candidate_with_its_subjects() {
    let c = Candidates {
        senders: vec![gemini()],
        subjects: vec![],
    };
    assert!(
        user_prompt("delete gemini notes", &[], &c, None)
            .contains("- sender Gemini <gemini@google.example> · 40 threads · subjects \"Notes:\"")
    );
}

#[rstest]
fn the_prompt_carries_the_rounds() {
    let rounds = vec![("Which notes?".to_string(), "the meeting ones".to_string())];
    assert!(
        user_prompt("delete notes", &rounds, &Candidates::default(), None)
            .contains("You asked: Which notes?\nThe user answered: the meeting ones")
    );
}

#[rstest]
fn the_prompt_says_what_was_picked() {
    let pick = Pick::SubjectPrefix("Notes:".into());
    assert!(
        user_prompt("delete notes", &[], &Candidates::default(), Some(&pick))
            .contains("The user picked the subject starting with \"Notes:\"")
    );
}

#[rstest]
fn a_pick_replaces_the_sender_and_words() {
    let mut p = Plan {
        steps: vec![Step {
            filter: Filter {
                from: vec!["gemini notes".into()],
                query: Some("meet".into()),
                older_than: Some(Age {
                    amount: 1,
                    unit: Unit::Year,
                }),
                ..Default::default()
            },
            action: Action::Delete,
        }],
    };
    Pick::From("gemini@google.example".into()).apply(&mut p);
    assert_eq!(
        p.steps[0].filter,
        Filter {
            from: vec!["gemini@google.example".into()],
            older_than: Some(Age {
                amount: 1,
                unit: Unit::Year
            }),
            ..Default::default()
        }
    );
}

#[rstest]
#[case::delete("delete all the messages like notes from gemini", Some(Action::Delete))]
#[case::french("supprime les notes de gemini", Some(Action::Delete))]
#[case::archive("archive the club mail", Some(Action::SetStatus { status: "archive".into() }))]
#[case::unread("mark them unread", Some(Action::MarkUnread))]
#[case::read("mark the newsletters read", Some(Action::MarkRead))]
#[case::unknown("tidy up gemini", None)]
fn the_verb_names_the_action(#[case] sentence: &str, #[case] want: Option<Action>) {
    assert_eq!(action_from_sentence(sentence), want);
}
