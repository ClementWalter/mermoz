//! Fictional demo mail (the validated sketches' content plus one thread per feed card
//! kind) for UI work, screenshots and end-to-end tests, without touching a real mailbox.
//! Every name, company and address here is made up.

use crate::Result;
use crate::mime::{ParsedAttachment, ParsedMessage};
use crate::model::{Address, Draft, new_id, now_ms};
use crate::store::{IncomingMessage, Store, SyncMode, ThreadPatch};

const H: i64 = 3_600_000;

fn addr(name: &str, email: &str) -> Address {
    Address {
        name: name.into(),
        email: email.into(),
    }
}

struct Seed<'a> {
    tid: &'a str,
    from: Address,
    subject: &'a str,
    body: &'a str,
    hours_ago: i64,
    labels: &'a [&'a str],
    attachments: Vec<ParsedAttachment>,
}

/// Fills an empty store with two demo accounts and their mail.
pub fn seed(store: &Store) -> Result<()> {
    let me = "you@work.example";
    let work = store.add_account(me, "gmail")?;
    store.set_sync(&work.id, "idle", Some(0), Some(0), None)?;
    let personal = store.add_account("you@personal.example", "gmail")?;
    store.set_sync(&personal.id, "idle", Some(0), Some(0), None)?;
    let you = addr("You", me);
    let you_home = addr("You", "you@personal.example");
    // A date two days ahead, so the Backlog event always comes back in the feed.
    let soon = chrono::Local::now() + chrono::Duration::days(2);
    let meeting = format!(
        "Bonjour,\n\nLa réunion parents-professeurs aura lieu le {} à 18h, salle 12. Merci d'apporter le carnet signé.\n\nMme Dubois",
        soon.format("%d/%m")
    );
    // Due five days ahead: within the week, past the two days that make a bill urgent.
    let bill = format!(
        "Bonjour,\n\nMontant à régler : 126,00 € avant le {}. Facture n° 2026-10-4471.\nPaiement en ligne ou par virement.",
        (chrono::Local::now() + chrono::Duration::days(5)).format("%d/%m")
    );
    let lease = ParsedAttachment {
        filename: "Avenant_bail_2026.pdf".into(),
        mime: "application/pdf".into(),
        data: b"%PDF-1.4 demo".to_vec(),
        ..Default::default()
    };
    let pdf = ParsedAttachment {
        filename: "audit-report-v2.pdf".into(),
        mime: "application/pdf".into(),
        data: b"%PDF-1.4 demo".to_vec(),
        ..Default::default()
    };
    let xls = ParsedAttachment {
        filename: "findings.xlsx".into(),
        mime: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
        data: b"demo".to_vec(),
        ..Default::default()
    };

    let seeds = vec![
        (
            &work,
            Seed {
                tid: "audit",
                from: addr("Marc Petit", "marc@work.example"),
                subject: "Audit report v2 — sign-off before Friday",
                body: "Hi,\n\nHere's the first draft, comments welcome.\n\nMarc",
                hours_ago: 120,
                labels: &[],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "audit",
                from: you.clone(),
                subject: "Re: Audit report v2 — sign-off before Friday",
                body: "Thanks — a few notes on §3 and §5 inline.",
                hours_ago: 72,
                labels: &["SENT"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "audit",
                from: addr("Marc Petit", "marc@work.example"),
                subject: "Re: Audit report v2 — sign-off before Friday",
                body: "Hi,\n\nAttached is v2 with your comments addressed. The main changes are in section 3 (scope of the key-management review) and section 5 (remediation timeline).\n\nCould you sign off by Friday? I'd like to send it to the board on Monday.\n\nThanks,\nMarc",
                hours_ago: 2,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![pdf, xls],
            },
        ),
        (
            &personal,
            Seed {
                tid: "lease",
                from: addr("Léa Dubois", "lea@agency.example"),
                subject: "Lease renewal — signed copy needed today",
                body: "Hello,\n\nPlease sign the attached renewal and send it back before 6pm.\n\nLéa",
                hours_ago: 1,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "interview",
                from: addr("Sofia Reyes", "sofia@candidate.example"),
                subject: "Re: Senior engineer interview — next steps",
                body: "Thanks for the chat yesterday! Happy to do the next round any time next week.",
                hours_ago: 20,
                labels: &["INBOX"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "budget",
                from: addr("Jonas Weber", "jonas@work.example"),
                subject: "Budget sheet for Q1 planning",
                body: "Could each team lead fill in their line in the attached sheet by Wednesday?",
                hours_ago: 50,
                labels: &["INBOX"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "viewing",
                from: addr("Agence Martin", "contact@agence.example"),
                subject: "Viewing confirmed for Saturday 10:00",
                body: "Your viewing is confirmed for Saturday at 10:00.",
                hours_ago: 70,
                labels: &["INBOX"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "bank",
                from: addr("Bank", "noreply@bank.example"),
                subject: "Your October statement is ready",
                body: "Your statement is available in your online banking.",
                hours_ago: 5,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "intro",
                from: addr("Nora Lind", "nora@partner.example"),
                subject: "Partnership intro — 20 min call?",
                body: "Hi! A mutual friend suggested we talk. Would you have 20 minutes this week?",
                hours_ago: 6,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "digest",
                from: addr("Product digest", "digest@news.example"),
                subject: "Weekly product digest",
                body: "This week: three launches, two deep dives and one long read.",
                hours_ago: 30,
                labels: &["INBOX"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "offer",
                from: addr("HR team", "hr@work.example"),
                subject: "Offer letter draft for Sofia",
                body: "Draft attached, can you review the compensation section?",
                hours_ago: 26,
                labels: &["INBOX"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "claim",
                from: you.clone(),
                subject: "Insurance claim — documents sent",
                body: "Please find the requested documents attached.",
                hours_ago: 216,
                labels: &["SENT"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "redlines",
                from: you.clone(),
                subject: "Contract redlines sent to vendor",
                body: "Here are our redlines on the MSA.",
                hours_ago: 40,
                labels: &["SENT"],
                attachments: vec![],
            },
        ),
        // One thread per feed card kind (all fictional).
        (
            &personal,
            Seed {
                tid: "tiles",
                from: addr("Marie Lefèvre", "marie@atelier-lefevre.example"),
                subject: "Carrelage cuisine : ton choix avant vendredi",
                body: "Bonjour,\n\nOn part sur lequel ? Le grès cérame gris (42 €/m², livré en 3 semaines) ou la terre cuite (58 €/m², 6 semaines) ? Il me faut une réponse vendredi pour tenir le planning.\n\nMarie",
                hours_ago: 3,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "canteen",
                from: addr("Caisse des écoles", "factures@ecoles.example"),
                subject: "Facture cantine — octobre",
                body: &bill,
                hours_ago: 8,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "school",
                from: addr("Mme Dubois", "secretariat@ecole-ferry.example"),
                subject: "Réunion parents-professeurs",
                body: &meeting,
                hours_ago: 300,
                labels: &["INBOX"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "lease-amendment",
                from: addr("Sophie Martin", "sophie@agence-martin.example"),
                subject: "Avenant au bail à signer",
                body: "Bonjour,\n\nVoici l'avenant au bail : le loyer passe de 1 240 € à 1 275 € au 1er novembre, charges inchangées. Merci de le signer page 4 et de parapher chaque page.\n\nSophie Martin",
                hours_ago: 30,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![lease],
            },
        ),
        (
            &work,
            Seed {
                tid: "figures",
                from: addr("Inès Garnier", "ines@work.example"),
                subject: "Chiffres du T3",
                body: "Salut, tu peux m'envoyer les chiffres du T3 pour le budget ?",
                hours_ago: 100,
                labels: &["INBOX"],
                attachments: vec![],
            },
        ),
        (
            &work,
            Seed {
                tid: "figures",
                from: you.clone(),
                subject: "Re: Chiffres du T3",
                body: "Oui, je t'envoie les chiffres du T3 lundi au plus tard.",
                hours_ago: 96,
                labels: &["SENT"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "plumber",
                from: you_home.clone(),
                subject: "Devis cuisine",
                body: "Bonjour M. Roux,\n\nPourriez-vous m'envoyer le devis pour la cuisine ?\n\nMerci",
                hours_ago: 144,
                labels: &["SENT"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "brief",
                from: addr("Le Brief Éco", "newsletter@brief-eco.example"),
                subject: "Le Brief Éco — semaine 42",
                body: "Cette semaine : la banque centrale maintient ses taux ; les prix des appartements reculent de 1,2 % sur un an.",
                hours_ago: 20,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "quartier",
                from: addr("Quartier Libre", "news@quartier-libre.example"),
                subject: "Quartier Libre — brocante samedi",
                body: "Brocante rue des Lilas samedi de 9h à 18h. Venez nombreux !",
                hours_ago: 26,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "ride",
                from: addr("Citymove", "no-reply@citymove.example"),
                subject: "Votre reçu Citymove",
                body: "Merci pour votre trajet. Total : 14,60 €, déjà débité.",
                hours_ago: 40,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![],
            },
        ),
        (
            &personal,
            Seed {
                tid: "order",
                from: addr("Maison & Co", "noreply@maisonco.example"),
                subject: "Commande 48211 expédiée",
                body: "Votre commande 48211 est en route. Livraison prévue sous 3 jours.",
                hours_ago: 50,
                labels: &["INBOX", "UNREAD"],
                attachments: vec![],
            },
        ),
    ];
    for (i, (acc, s)) in seeds.into_iter().enumerate() {
        let msg = IncomingMessage {
            provider_id: format!("demo{i}"),
            provider_thread_id: s.tid.into(),
            label_ids: s.labels.iter().map(|l| l.to_string()).collect(),
            internal_date: now_ms() - s.hours_ago * H,
            parsed: ParsedMessage {
                from: Some(s.from),
                to: vec![addr("You", &acc.email)],
                subject: s.subject.into(),
                body_text: s.body.into(),
                message_id: Some(format!("<demo{i}@demo.example>")),
                attachments: s.attachments,
                ..Default::default()
            },
        };
        store.ingest(&acc.id, &acc.email, &msg, SyncMode::Initial)?;
    }
    // Older mail the user archived from the Feed: three yes per sender teach Mermoz to
    // archive them, so their new mail is cleared together.
    let archived = [
        (
            addr("Citymove", "no-reply@citymove.example"),
            "Votre reçu Citymove",
        ),
        (
            addr("Le Brief Éco", "newsletter@brief-eco.example"),
            "Le Brief Éco",
        ),
        (
            addr("Quartier Libre", "news@quartier-libre.example"),
            "Quartier Libre",
        ),
    ];
    for (s, (from, subject)) in archived.iter().enumerate() {
        for week in 1..=3 {
            let msg = IncomingMessage {
                provider_id: format!("old{s}-{week}"),
                provider_thread_id: format!("old{s}-{week}"),
                label_ids: vec![],
                internal_date: now_ms() - week * 7 * 24 * H,
                parsed: ParsedMessage {
                    from: Some(from.clone()),
                    to: vec![addr("You", "you@personal.example")],
                    subject: format!("{subject} (archive)"),
                    body_text: "Old issue.".into(),
                    message_id: Some(format!("<old{s}-{week}@demo.example>")),
                    ..Default::default()
                },
            };
            let tid = store.ingest(&personal.id, &personal.email, &msg, SyncMode::Initial)?;
            store.learn_archive(&[tid])?;
        }
    }
    // A conversation spread over three Gmail threads with Nora (the user wrote first),
    // and two automated mails from one sender: grouping shows them as one chat and one feed.
    let nora = addr("Nora Lind", "nora@agency.example");
    let sale = [
        (
            "sale1",
            you.clone(),
            nora.clone(),
            "Flat sale, next steps",
            "Hi Nora,\n\nHere is the cellar plan. The cellar is lot 14, not 12 as in the draft.\n\nThanks!",
            150,
            vec!["SENT"],
            Some(ParsedAttachment {
                filename: "cellar-plan.pdf".into(),
                mime: "application/pdf".into(),
                data: b"%PDF-1.4 demo".to_vec(),
                ..Default::default()
            }),
        ),
        (
            "sale2",
            nora.clone(),
            you.clone(),
            "Signing postponed",
            "Hello,\n\nThe signing moves to the 20th: the property description must be corrected first. You can pick a slot here: https://notary.example/booking\n\nNora",
            96,
            vec!["INBOX"],
            None,
        ),
        (
            "sale3",
            nora.clone(),
            you.clone(),
            "Photo of the cellar door",
            "As promised, the cellar door after the repair.",
            20,
            vec!["INBOX", "UNREAD"],
            Some(ParsedAttachment {
                filename: "cellar-door.png".into(),
                mime: "image/png".into(),
                data: b"\x89PNG demo".to_vec(),
                ..Default::default()
            }),
        ),
        (
            "bank1",
            addr("Demo Bank", "noreply@bank.example"),
            you.clone(),
            "Your October statement",
            "Your statement is ready.",
            70,
            vec!["INBOX"],
            None,
        ),
        (
            "bank2",
            addr("Demo Bank", "noreply@bank.example"),
            you.clone(),
            "Card payment of 42.00",
            "A card payment was made.",
            10,
            vec!["INBOX", "UNREAD"],
            None,
        ),
    ];
    for (i, (tid, from, to, subject, body, hours, labels, file)) in sale.into_iter().enumerate() {
        let msg = IncomingMessage {
            provider_id: format!("conv{i}"),
            provider_thread_id: tid.into(),
            label_ids: labels.iter().map(|l| l.to_string()).collect(),
            internal_date: now_ms() - hours * H,
            parsed: ParsedMessage {
                from: Some(from),
                to: vec![to],
                subject: subject.into(),
                body_text: body.into(),
                message_id: Some(format!("<conv{i}@demo.example>")),
                attachments: file.into_iter().collect(),
                ..Default::default()
            },
        };
        store.ingest(&work.id, me, &msg, SyncMode::Initial)?;
    }
    for t in store.threads_to_group("pending", 1000, false)? {
        store.classify_for_grouping(&t)?;
    }
    let sale_ids: Vec<String> = store
        .threads(&Default::default())?
        .0
        .into_iter()
        .filter(|t| {
            ["Flat sale", "Signing postponed", "Photo of the cellar"]
                .iter()
                .any(|p| t.subject.starts_with(p))
        })
        .map(|t| t.id)
        .collect();
    store.merge_threads(&sale_ids)?;

    let q4 = store.create_project("Q4 audit", Some("#5B8DEF"))?;
    let hiring = store.create_project("Hiring", Some("#E0782F"))?;
    let house = store.create_project("House", Some("#2E9A6B"))?;
    let set = |n: &str, p: ThreadPatch| -> Result<()> {
        let id = store.resolve_thread_id(n)?;
        store.update_threads(&[id], &p)?;
        Ok(())
    };
    let tid = |subject: &str| -> Result<String> {
        store
            .threads(&Default::default())?
            .0
            .into_iter()
            .find(|t| t.subject.starts_with(subject))
            .map(|t| t.number)
            .ok_or_else(|| crate::Error::NotFound(subject.to_string()))
    };
    set(
        &tid("Audit report")?,
        ThreadPatch {
            status: Some("todo".into()),
            priority: Some(2),
            add_projects: vec![q4.id.clone()],
            due: Some(Some("2026-10-16".into())),
            ..Default::default()
        },
    )?;
    set(
        &tid("Lease renewal")?,
        ThreadPatch {
            status: Some("todo".into()),
            priority: Some(1),
            add_projects: vec![house.id.clone()],
            ..Default::default()
        },
    )?;
    set(
        &tid("Re: Senior")?,
        ThreadPatch {
            status: Some("todo".into()),
            priority: Some(2),
            add_projects: vec![hiring.id.clone()],
            ..Default::default()
        },
    )?;
    set(
        &tid("Budget sheet")?,
        ThreadPatch {
            status: Some("todo".into()),
            priority: Some(2),
            ..Default::default()
        },
    )?;
    set(
        &tid("Viewing")?,
        ThreadPatch {
            status: Some("todo".into()),
            priority: Some(3),
            add_projects: vec![house.id],
            ..Default::default()
        },
    )?;
    set(
        &tid("Offer letter")?,
        ThreadPatch {
            status: Some("in_progress".into()),
            priority: Some(3),
            add_projects: vec![hiring.id],
            ..Default::default()
        },
    )?;
    set(
        &tid("Insurance claim")?,
        ThreadPatch {
            status: Some("waiting".into()),
            due: Some(Some("2026-10-09".into())),
            ..Default::default()
        },
    )?;
    set(
        &tid("Contract redlines")?,
        ThreadPatch {
            status: Some("waiting".into()),
            add_projects: vec![q4.id],
            due: Some(Some("2026-10-14".into())),
            ..Default::default()
        },
    )?;
    let kitchen = store.create_project("Kitchen", Some("#E0782F"))?;
    set(
        &tid("Carrelage cuisine")?,
        ThreadPatch {
            add_projects: vec![kitchen.id.clone()],
            ..Default::default()
        },
    )?;
    set(
        &tid("Devis cuisine")?,
        ThreadPatch {
            status: Some("waiting".into()),
            add_projects: vec![kitchen.id],
            ..Default::default()
        },
    )?;
    set(
        &tid("Réunion parents")?,
        ThreadPatch {
            status: Some("backlog".into()),
            ..Default::default()
        },
    )?;
    set(
        &tid("Chiffres du T3")?,
        ThreadPatch {
            status: Some("todo".into()),
            ..Default::default()
        },
    )?;
    // An agent's draft waiting for approval, as in the Review sketch.
    let budget = store.resolve_thread_id(&tid("Budget sheet")?)?;
    let d = Draft {
        id: new_id("dft"),
        account_id: work.id.clone(),
        thread_id: Some(budget),
        to: vec!["jonas@work.example".into()],
        cc: vec!["marc@work.example".into()],
        subject: "Re: Budget sheet for Q1 planning".into(),
        body: "Hi Jonas,\n\nI've filled in our team's line: headcount stays flat, and the tooling budget goes up to cover the new CI runners. Happy to walk you through it on Thursday.\n\nBest,".into(),
        state: "draft".into(),
        origin: "agent".into(),
        created: now_ms(),
        updated: now_ms(),
        attachments: vec![],
        reply_to_message_id: None,
    };
    store.create_draft(&d)?;
    store.move_to_review(&d.id)?;
    Ok(())
}
