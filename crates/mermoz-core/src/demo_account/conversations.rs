//! The demo's conversations with people: Maya Laurent runs Fernhill Studio, a small
//! product design studio in Lyon, with her co-founder Tom. Clients (Northbeam Bikes,
//! Atelier Sève), candidates, an accountant, family and friends. Every name, company
//! and address is made up.

use super::When::{Day, Mins};
use super::{Mail, Plan, When, art, chip, choice, me, person};
use crate::feed::{Judgment, Kind, Texts};
use crate::mime::ParsedAttachment;
use crate::model::Address;

pub(crate) const PEOPLE: &[super::Person] = &[
    p("tom", "Tom Becker", "tom@fernhill.example"),
    p("ines", "Inès Moreau", "ines@fernhill.example"),
    p("raj", "Raj Patel", "raj@patelworks.example"),
    p("clara", "Clara Dupont", "clara@dupont-compta.example"),
    p("ellen", "Ellen Shaw", "ellen.shaw@northbeam.example"),
    p("marcus", "Marcus Lee", "marcus.lee@northbeam.example"),
    // A mistyped address for Marcus: mail to it bounces.
    p("marcos", "Marcus Lee", "marcos.lee@northbeam.example"),
    p("hana", "Hana Kim", "hana.kim@northbeam.example"),
    p("dan", "Dan Okafor", "dan@kestrel-outdoor.example"),
    p("camille", "Camille Roux", "camille@atelier-seve.example"),
    p("priya", "Priya Nair", "priya.nair@mail.example"),
    p("hugo", "Hugo Martin", "hugo.martin@mail.example"),
    p("sofia", "Sofia Alvarez", "sofia@harbor-ventures.example"),
    p("lea", "Léa Fontaine", "lea@designdays-lyon.example"),
    p(
        "garnier",
        "Imprimerie Garnier",
        "devis@garnier-print.example",
    ),
    p("julien", "Julien Laurent", "julien.laurent@mail.example"),
    p("elise", "Élise Laurent", "elise.laurent@mail.example"),
    p("nadia", "Nadia Haddad", "nadia.haddad@mail.example"),
    p("sam", "Sam Okoye", "sam.okoye@mail.example"),
    p("gerard", "Gérard Blanc", "g.blanc@immo-blanc.example"),
    p(
        "ecole",
        "École des Tilleuls",
        "secretariat@ecole-tilleuls.example",
    ),
    p(
        "bruno",
        "Bruno Ferreira",
        "bruno@ferreira-renovation.example",
    ),
    p(
        "horizon",
        "Assurance Horizon",
        "sinistres@assurance-horizon.example",
    ),
    p("helene", "Hélène Vidal", "h.vidal@ecole-design.example"),
    p(
        "cabinet",
        "Cabinet dentaire Morel",
        "rdv@cabinet-morel.example",
    ),
];

const fn p(key: &'static str, name: &'static str, email: &'static str) -> super::Person {
    super::Person { key, name, email }
}

fn addr(key: &str) -> Address {
    if key == "me" {
        me()
    } else {
        person(key).address()
    }
}

/// A date `days` from the demo's day, as the Feed stores it.
pub(crate) fn date_in(now: i64, days: i64) -> String {
    (chrono::DateTime::from_timestamp_millis(now)
        .unwrap_or_default()
        .with_timezone(&chrono::Local)
        .date_naive()
        + chrono::Duration::days(days))
    .to_string()
}

/// One conversation, written message by message.
struct Conv {
    key: &'static str,
    subject: &'static str,
    /// The people in it besides Maya; the first is who she writes to.
    with: Vec<&'static str>,
    cc: Vec<&'static str>,
    msgs: Vec<(&'static str, When, String, Vec<ParsedAttachment>)>,
    folders: Vec<&'static str>,
    unread: bool,
    inbox: bool,
    starred: bool,
    plan: Plan,
}

fn conv(key: &'static str, subject: &'static str, with: &[&'static str]) -> Conv {
    Conv {
        key,
        subject,
        with: with.to_vec(),
        cc: vec![],
        msgs: vec![],
        folders: vec![],
        unread: false,
        inbox: true,
        starred: false,
        plan: Plan {
            thread: key.into(),
            ..Default::default()
        },
    }
}

impl Conv {
    fn say(mut self, who: &'static str, when: When, body: &str) -> Self {
        self.msgs.push((who, when, body.trim().to_string(), vec![]));
        self
    }

    fn file(mut self, f: ParsedAttachment) -> Self {
        if let Some(m) = self.msgs.last_mut() {
            m.3.push(f);
        }
        self
    }

    fn cc(mut self, cc: &[&'static str]) -> Self {
        self.cc = cc.to_vec();
        self
    }

    fn folder(mut self, f: &'static str) -> Self {
        self.folders.push(f);
        self
    }

    fn unread(mut self) -> Self {
        self.unread = true;
        self
    }

    fn archived(mut self) -> Self {
        self.inbox = false;
        self
    }

    fn starred(mut self) -> Self {
        self.starred = true;
        self.plan.priority = Some(2);
        self
    }

    fn status(mut self, s: &'static str) -> Self {
        self.plan.status = Some(s);
        self
    }

    fn later(mut self, hours: i64) -> Self {
        self.plan.later_hours = Some(hours);
        self
    }

    fn judged(mut self, j: Judgment) -> Self {
        self.plan.judgment = Some(j);
        self
    }

    fn card(mut self, t: Texts) -> Self {
        self.plan.card = Some(t);
        self
    }

    fn chip(mut self, status: Option<&str>, folders: &[&str]) -> Self {
        self.plan.chip = Some(chip(status, folders));
        self
    }

    fn gist(mut self, summary: &str, next: &str) -> Self {
        self.plan.gist = Some((summary.into(), next.into()));
        self
    }

    fn done(self, now: i64, mails: &mut Vec<Mail>, plans: &mut Vec<Plan>) {
        let last = self.msgs.len().saturating_sub(1);
        for (i, (who, when, body, files)) in self.msgs.into_iter().enumerate() {
            let subject = if i == 0 {
                self.subject.to_string()
            } else {
                format!("Re: {}", self.subject)
            };
            let mut m = Mail::new(self.key, addr(who), &subject, body, when.at(now));
            m.to = if who == "me" {
                self.with.iter().take(1).map(|k| addr(k)).collect()
            } else {
                std::iter::once(me())
                    .chain(self.with.iter().filter(|k| **k != who).map(|k| addr(k)))
                    .collect()
            };
            if who == "me" && self.with.len() > 1 {
                m.to.extend(self.with.iter().skip(1).map(|k| addr(k)));
            }
            m.cc = self.cc.iter().map(|k| addr(k)).collect();
            m.files = files;
            m.unread = self.unread && i == last;
            m.inbox = self.inbox;
            m.starred = self.starred;
            m.folders = self.folders.clone();
            mails.push(m);
        }
        plans.push(self.plan);
    }
}

fn judgment(kind: Kind, needs_reply: f64, urgency: f64) -> Judgment {
    Judgment {
        kind: Some(kind),
        needs_reply: Some(needs_reply),
        urgency: Some(urgency),
        automated: Some(0.03),
        sender_weight: 0.7,
        typical_reply_days: Some(1.5),
        ..Default::default()
    }
}

fn texts(title: &str, why: &str, summary: &str) -> Texts {
    Texts {
        title: title.into(),
        why: why.into(),
        summary: summary.into(),
        ..Default::default()
    }
}

fn pdf(name: &str, title: &str, lines: &[&str]) -> ParsedAttachment {
    ParsedAttachment {
        filename: name.into(),
        mime: "application/pdf".into(),
        data: art::pdf(
            title,
            &lines.iter().map(|l| l.to_string()).collect::<Vec<_>>(),
        ),
        ..Default::default()
    }
}

fn png(name: &str, data: Vec<u8>) -> ParsedAttachment {
    ParsedAttachment {
        filename: name.into(),
        mime: "image/png".into(),
        data,
        ..Default::default()
    }
}

/// Every conversation with people, oldest matters first.
pub(crate) fn build(now: i64, mails: &mut Vec<Mail>, plans: &mut Vec<Plan>) {
    for c in clients(now)
        .into_iter()
        .chain(studio(now))
        .chain(home(now))
        .chain(history())
    {
        c.done(now, mails, plans);
    }
    mails.push(bounce(now));
}

fn clients(now: i64) -> Vec<Conv> {
    vec![
        conv("nb-kickoff", "Northbeam onboarding redesign: kickoff notes", &["ellen"])
            .cc(&["marcus"])
            .folder("Clients/Northbeam")
            .say("ellen", Day(24, 9, 12), "
Hi Maya,

Great to finally get started! As promised, a recap of what we agreed on today:

- Scope: the first-ride onboarding (account, bike pairing, first route), mobile only.
- Timeline: 6 weeks, with a review every other Thursday.
- Success metric: more riders finishing pairing on day one (today it's 61%).

Marcus will send you access to the analytics dashboard and last year's research.

Talk soon,
Ellen")
            .say("me", Day(24, 14, 40), "
Thanks Ellen, that matches my notes.

One addition: we'd like to sit in on two or three support calls in the first week, to hear where riders get stuck in their own words. Could you set that up with your support lead?

Maya")
            .say("ellen", Day(23, 10, 5), "
Done, Hana from support will invite you to Tuesday's and Wednesday's calls.

Ellen")
            .say("me", Day(23, 10, 31), "Perfect, thank you!")
            .status("done")
            .gist(
                "Northbeam onboarding redesign: 6 weeks, mobile, reviews every other Thursday. Goal: more riders finishing bike pairing on day one (61% today).",
                "Nothing to do: kickoff is wrapped up.",
            ),
        conv("nb-review", "Ride tracking screens: feedback round 2", &["ellen", "marcus"])
            .folder("Clients/Northbeam")
            .starred()
            .unread()
            .say("me", Day(3, 17, 20), "
Hi both,

Round 2 of the ride tracking screens is up on the board. Main changes since last time:

1. The live map takes the full screen; stats slide up from the bottom.
2. Pause and stop are separated, so nobody ends a ride by accident.
3. Two navigation options to compare (A: bottom tabs, B: side drawer).

Prototype link and notes are on the board. Looking forward to your thoughts!

Maya")
            .say("marcus", Day(2, 9, 48), "
Love the full-screen map, riders will too. The pause/stop split solves the #1 complaint in the app store reviews.

On navigation I lean towards A, the drawer hides too much on small phones.

Marcus")
            .say("me", Day(2, 11, 3), "
Thanks Marcus! Ellen, do you agree with A? Inès needs a decision to start the detailed specs.")
            .say("ellen", Mins(185), "
Sorry for the slow answer, board meeting week.

I like both, honestly. A is clearer, but B leaves room for the community features we're planning next year. Could you make the call? Whatever you pick, we need the final screens by Thursday to keep next week's test session with riders.

Ellen")
            .judged(Judgment {
                due: Some(date_in(now, 3)),
                ..judgment(Kind::Decision, 0.92, 2.6)
            })
            .card(Texts {
                options: vec![
                    choice(
                        "Bottom tabs (A)",
                        "Clearer on small phones; Marcus prefers it",
                        true,
                        "It is the safer choice for the rider test, and community features can get a tab later.",
                        "Hi Ellen,\n\nLet's go with A, the bottom tabs: it tested clearer on small phones, and community can become a fifth tab next year without a redesign. Final screens on Thursday as planned.\n\nMaya",
                    ),
                    choice(
                        "Side drawer (B)",
                        "Leaves room for next year's community features",
                        false,
                        "",
                        "Hi Ellen,\n\nLet's go with B, the side drawer: it gives the community features room to grow next year. Final screens on Thursday as planned.\n\nMaya",
                    ),
                ],
                points: vec![
                    "Ellen leaves the navigation choice to you".into(),
                    "Final screens are due Thursday for the rider test".into(),
                ],
                ..texts(
                    "Choose the navigation for Northbeam",
                    "Ellen needs final screens by Thursday for the rider test.",
                    "Ellen likes both options and asks you to decide; Marcus prefers bottom tabs.",
                )
            })
            .gist(
                "Round 2 of the ride tracking screens: full-screen map, pause and stop split. Marcus prefers bottom tabs (A); Ellen leaves the choice to you.",
                "Pick A or B today so the final screens are ready Thursday.",
            ),
        conv("nb-invoice", "Invoice FS-2026-041: Northbeam, milestone 1", &["marcus"])
            .folder("Clients/Northbeam")
            .folder("Finance")
            .say("me", Day(9, 10, 2), "
Hi Marcus,

Please find attached the invoice for the first milestone of the onboarding redesign (research and concepts).

Amount: 8,400.00 EUR, payable within 30 days.

Thanks!
Maya")
            .file(pdf(
                "FS-2026-041.pdf",
                "Fernhill Studio - Invoice FS-2026-041",
                &[
                    "Billed to: Northbeam Bikes, accounts payable",
                    "Project: onboarding redesign, milestone 1",
                    "Research and concepts ........ 8,400.00 EUR",
                    "VAT reverse charge",
                    "Total due ..................... 8,400.00 EUR",
                    "Payment within 30 days. IBAN FR76 0000 0000 0000 0000 0000 000",
                ],
            ))
            .say("marcus", Day(8, 15, 37), "
Got it, forwarded to finance. They pay at the end of the month, so you should see it around the 30th.

Marcus")
            .status("waiting")
            .judged(judgment(Kind::Waiting, 0.1, 0.8))
            .card(texts(
                "Northbeam pays invoice FS-2026-041",
                "Finance pays at the end of the month.",
                "Marcus forwarded the 8,400 EUR invoice to finance; payment expected around the 30th.",
            ))
            .gist(
                "Invoice FS-2026-041 (8,400 EUR, milestone 1) sent to Northbeam; finance pays at the end of the month.",
                "Check the payment arrived around the 30th.",
            ),
        conv("nb-bounce", "Prototype link for Thursday's test", &["marcos"])
            .folder("Clients/Northbeam")
            .judged(judgment(Kind::Reply, 0.7, 2.2))
            .card(Texts {
                draft: "Hi Marcus,\n\nResending this, my first email went to a mistyped address. Here is the prototype for Thursday's rider test.\n\nMaya".into(),
                ..texts(
                    "Resend the prototype link to Marcus",
                    "Your email didn't arrive: the address had a typo.",
                    "The prototype link bounced: it went to marcos.lee@ instead of marcus.lee@.",
                )
            })
            .say("me", Mins(26 * 60), "
Hi Marcus,

Here is the prototype for Thursday's rider test, as promised. It works on any phone, no install needed.

Prototype: Northbeam ride tracking, v2.3 (link in the board)

Maya"),
        conv("nb-contract", "Northbeam: framework agreement 2027", &["marcus"])
            .folder("Clients/Northbeam")
            .unread()
            .say("marcus", Mins(7 * 60 + 15), "
Hi Maya,

Legal finished the framework agreement for next year (same day rate, 60 days of design work, renewable). Could you sign it by next Monday? Our budget closes that day.

Signature page is page 4.

Best,
Marcus")
            .file(pdf(
                "Northbeam-Fernhill-agreement-2027.pdf",
                "Framework agreement 2027",
                &[
                    "Between Northbeam Bikes and Fernhill Studio",
                    "Term: 1 January 2027 - 31 December 2027",
                    "Volume: 60 days of product design, renewable",
                    "Day rate: unchanged from 2026",
                    "Signatures: page 4",
                ],
            ))
            .judged(Judgment {
                due: Some(date_in(now, 5)),
                ..judgment(Kind::Sign, 0.6, 2.1)
            })
            .card(Texts {
                draft: "Hi Marcus,\n\nThanks! I'll read it tonight and send it back signed before Monday.\n\nMaya".into(),
                points: vec![
                    "Same day rate, 60 days, renewable".into(),
                    "Signature on page 4, before Monday".into(),
                ],
                ..texts(
                    "Sign Northbeam's 2027 agreement",
                    "Their budget closes Monday.",
                    "Marcus sends next year's framework agreement: same rate, 60 days of design work.",
                )
            })
            .gist(
                "Northbeam's framework agreement for 2027: same day rate, 60 days, renewable. Signature on page 4.",
                "Sign and return before Monday.",
            ),
        conv("dan-intro", "Intro: Maya <> Dan (Kestrel Outdoor)", &["ellen", "dan"])
            .unread()
            .say("ellen", Mins(20 * 60), "
Maya, meet Dan, head of digital at Kestrel Outdoor. They're rethinking their trail app and I told him about the work you've been doing with us.

Dan, meet Maya, co-founder of Fernhill Studio, the team behind our new onboarding.

I'll let you two take it from here!
Ellen")
            .say("dan", Mins(17 * 60), "
Thanks Ellen! Maya, great to meet you. Would you have 30 minutes next week for a call? Tuesday or Wednesday afternoon work best for me.

Dan")
            .judged(judgment(Kind::Intro, 0.88, 1.8))
            .card(Texts {
                draft: "Hi Dan,\n\nLovely to meet you, and thanks Ellen for the intro (moving you to bcc).\n\nWednesday at 3pm works for me. I'll send an invite with a call link.\n\nMaya".into(),
                ..texts(
                    "Answer Dan's intro from Ellen",
                    "A warm lead from a happy client.",
                    "Ellen introduces Dan (Kestrel Outdoor), who wants a 30-minute call next week.",
                )
            })
            .gist(
                "Ellen introduced Dan from Kestrel Outdoor; he'd like a 30-minute call Tuesday or Wednesday afternoon.",
                "Propose a slot to Dan.",
            ),
        conv("seve-packaging", "Emballages : nouvelle gamme été", &["camille"])
            .folder("Clients/Sève")
            .say("camille", Day(6, 10, 15), "
Bonjour Maya,

Comme convenu, voici le brief pour les emballages de la gamme été : trois produits (crème solaire, brume, après-soleil), un univers plus lumineux que la gamme actuelle, et toujours zéro plastique.

On aimerait voir deux pistes avant la fin du mois. Ça te paraît faisable ?

Belle journée,
Camille")
            .say("me", Day(6, 16, 2), "
Bonjour Camille,

Merci pour le brief, très clair ! Oui, deux pistes pour la fin du mois c'est faisable. Est-ce que tu peux m'envoyer les échantillons de papier dont tu parlais ?

Maya")
            .say("camille", Day(1, 11, 30), "
Les échantillons sont partis hier par coursier, et voici une photo en attendant. On a un coup de cœur pour le papier sable (à gauche).

Camille")
            .file(png(
                "echantillons-papier.png",
                art::tiles([233, 214, 180], [247, 236, 214], [210, 196, 170]),
            ))
            .status("todo")
            .judged(judgment(Kind::Reply, 0.7, 1.4))
            .card(Texts {
                draft: "Merci Camille, la photo donne déjà une bonne idée ! Le papier sable me plaît beaucoup aussi, je pars là-dessus pour la première piste.\n\nJe te dis dès que les échantillons sont arrivés.\n\nMaya".into(),
                ..texts(
                    "Confirme le papier sable à Camille",
                    "Elle attend ton avis pour lancer les pistes.",
                    "Les échantillons arrivent par coursier ; Camille préfère le papier sable.",
                )
            })
            .gist(
                "Gamme été d'Atelier Sève : trois emballages zéro plastique, deux pistes attendues fin du mois. Camille préfère le papier sable.",
                "Répondre à Camille et lancer la première piste.",
            ),
        conv("seve-devis", "Devis refonte du site : Atelier Sève", &["camille"])
            .folder("Clients/Sève")
            .unread()
            .say("me", Day(5, 9, 45), "
Bonjour Camille,

Voici notre devis pour la refonte du site (boutique, pages produits, journal). Six semaines, livraison début décembre.

N'hésite pas si tu as des questions !
Maya")
            .file(pdf(
                "Devis-Fernhill-Seve-site.pdf",
                "Devis - refonte du site Atelier Seve",
                &[
                    "Ateliers et recherche ........ 3 200 EUR",
                    "Design des pages ............. 7 800 EUR",
                    "Suivi du developpement ....... 2 400 EUR",
                    "Total HT .................... 13 400 EUR",
                    "Validite : 30 jours",
                ],
            ))
            .say("camille", Mins(28 * 60), "
Bonjour Maya,

C'est validé de notre côté ! Est-ce que tu peux nous envoyer le bon de commande à signer ? Notre comptable en a besoin pour le premier acompte.

Merci,
Camille")
            .judged(judgment(Kind::Reply, 0.9, 2.0))
            .card(Texts {
                draft: "Super nouvelle, merci Camille !\n\nJe t'envoie le bon de commande aujourd'hui ; l'acompte de 30 % lancera le projet.\n\nMaya".into(),
                ..texts(
                    "Envoie le bon de commande à Atelier Sève",
                    "Le devis est validé : l'acompte en dépend.",
                    "Camille valide le devis de 13 400 € et demande le bon de commande.",
                )
            })
            .chip(Some("todo"), &[]),
        conv("lea-talk", "Speaker invitation: Design Days Lyon", &["lea"])
            .say("lea", Day(3, 14, 22), "
Dear Maya,

I'm part of the team behind Design Days Lyon (two days, 600 designers, at the Docks). We'd love to have you on stage for a 25-minute talk about designing onboarding for hardware products.

The conference is in seven weeks. Could you let us know by the end of next week whether you're in? We cover travel and offer two tickets for your team.

Warm regards,
Léa Fontaine
Program team, Design Days Lyon")
            .judged(Judgment {
                due: Some(date_in(now, 8)),
                ..judgment(Kind::Decision, 0.85, 1.2)
            })
            .card(Texts {
                options: vec![
                    choice(
                        "Accept",
                        "25 minutes on onboarding for hardware products",
                        true,
                        "The Northbeam work is a strong story, and the audience is your future clients.",
                        "Dear Léa,\n\nThank you for the invitation, I'd be glad to speak! Could you send me the speaker guide and the date of the rehearsal?\n\nBest,\nMaya",
                    ),
                    choice(
                        "Decline",
                        "Too busy this autumn",
                        false,
                        "",
                        "Dear Léa,\n\nThank you so much for thinking of me. This autumn is too busy for me to prepare a talk properly, but I'd love to be considered next year.\n\nBest,\nMaya",
                    ),
                ],
                ..texts(
                    "Decide on the Design Days talk",
                    "They need an answer by the end of next week.",
                    "Léa invites you to give a 25-minute talk at Design Days Lyon; travel is covered.",
                )
            }),
        conv("garnier", "Devis impression : cartes de visite et papeterie", &["garnier"])
            .folder("Finance")
            .say("me", Day(8, 11, 0), "
Bonjour,

Pourriez-vous nous faire un devis pour 500 cartes de visite (4 personnes) et 1 000 enveloppes, papier recyclé ?

Merci,
Maya Laurent, Fernhill Studio")
            .say("garnier", Day(5, 16, 10), "
Bonjour Madame Laurent,

Veuillez trouver ci-joint notre devis. Délai de fabrication : 5 jours ouvrés après validation du BAT.

Cordialement,
L'équipe Imprimerie Garnier")
            .file(pdf(
                "Devis-Garnier-2026-118.pdf",
                "Imprimerie Garnier - Devis 2026-118",
                &[
                    "Cartes de visite, 4 x 125, papier recycle 350 g ... 186,00 EUR",
                    "Enveloppes 1000, papier recycle ................. 142,00 EUR",
                    "Total TTC ....................................... 328,00 EUR",
                ],
            ))
            .later(5 * 24)
            .judged(Judgment {
                amount: Some("328,00 €".into()),
                ..judgment(Kind::Payment, 0.4, 0.9)
            })
            .card(texts(
                "Valider le devis de l'imprimerie",
                "328 € pour les cartes et les enveloppes.",
                "Imprimerie Garnier envoie un devis de 328 € TTC, livraison 5 jours après le BAT.",
            )),
        conv("sofia-coffee", "Coffee next time you're in Paris?", &["sofia"])
            .say("sofia", Day(8, 8, 55), "
Hi Maya,

I loved your piece on designing for first rides, it's been shared a lot around the office. We're looking at a few hardware startups and I'd love to pick your brain on what makes their onboarding work.

Any chance you're in Paris in the coming weeks? Coffee is on me.

Sofia Alvarez
Harbor Ventures")
            .later(2 * 24)
            .judged(judgment(Kind::Reply, 0.6, 0.6))
            .card(Texts {
                draft: "Hi Sofia,\n\nThank you, glad it resonated! I'm in Paris next week for a client workshop: would coffee near Gare de Lyon at 9am work?\n\nMaya".into(),
                ..texts(
                    "Answer Sofia about coffee in Paris",
                    "No rush, but an investor asking for advice.",
                    "Sofia (Harbor Ventures) liked your article and suggests coffee in Paris.",
                )
            }),
    ]
}

fn studio(now: i64) -> Vec<Conv> {
    vec![
        conv("tom-shortlist", "Senior designer role: shortlist", &["tom"])
            .folder("Hiring")
            .say("tom", Day(7, 18, 2), "
Hey,

We're at 34 applications for the senior role. I went through all of them; my top five are in the doc. Priya and Hugo stand out, both strong in research.

Can you have a look before Friday?

T.")
            .say("me", Day(6, 9, 12), "
Looked at all five. Agree on Priya, her case study on the transit app is excellent. Hugo is good but feels more junior to me.

Let's invite Priya first and keep Hugo for the mid-level role in spring?")
            .say("tom", Day(6, 9, 40), "Works for me. I'll let you email Priya, you know her mentor.")
            .say("me", Day(6, 10, 5), "On it.")
            .status("in_progress")
            .judged(judgment(Kind::Promise, 0.4, 1.3))
            .card(texts(
                "Schedule Priya's interview",
                "You told Tom you'd email her.",
                "You and Tom shortlisted Priya first; Hugo is kept for the spring role.",
            ))
            .gist(
                "34 applications for the senior designer role. You and Tom agree on Priya first; Hugo is kept for the spring mid-level role.",
                "Schedule Priya's interview.",
            ),
        conv("priya", "Application: Senior Product Designer", &["priya"])
            .folder("Hiring")
            .unread()
            .say("priya", Day(9, 21, 15), "
Hello Maya,

I'm applying for the senior product designer role at Fernhill. I've spent the last four years designing ticketing and trip planning for a regional transit network, and I'm looking for a smaller studio where research and craft go together.

My portfolio is attached. Thank you for reading!

Priya Nair")
            .file(pdf(
                "Priya-Nair-portfolio.pdf",
                "Priya Nair - Portfolio",
                &[
                    "Transit app: buying a ticket in 3 taps (case study)",
                    "Trip planner for riders with reduced mobility",
                    "Design system for 4 regional apps",
                    "References on request",
                ],
            ))
            .say("me", Day(6, 10, 20), "
Hi Priya,

Thank you for applying, Tom and I really enjoyed your transit case study. Would you be available for a first conversation next week (about an hour, at the studio or by video)?

Best,
Maya")
            .say("priya", Mins(9 * 60 + 40), "
Hi Maya,

Thank you, I'd be delighted! I'm free Tuesday after 2pm or Thursday morning. I can come to the studio.

Priya")
            .judged(judgment(Kind::Reply, 0.95, 2.0))
            .card(Texts {
                draft: "Hi Priya,\n\nWonderful! Let's say Thursday at 10am at the studio (12 rue des Capucins, 2nd floor). Tom will join us.\n\nSee you then,\nMaya".into(),
                ..texts(
                    "Pick a time with Priya",
                    "Your top candidate is waiting for a slot.",
                    "Priya can come Tuesday after 2pm or Thursday morning.",
                )
            })
            .gist(
                "Priya, top candidate for the senior role (transit app background), is available Tuesday after 2pm or Thursday morning.",
                "Confirm a slot and invite Tom.",
            ),
        conv("hugo", "Candidature : designer produit", &["hugo"])
            .unread()
            .say("hugo", Day(4, 8, 30), "
Bonjour,

Je me permets de vous écrire pour le poste de designer produit. Je termine ma deuxième année chez une agence lyonnaise où je travaille surtout sur des applications de santé.

Vous trouverez mon portfolio en lien. Je serais ravi d'échanger avec vous !

Hugo Martin")
            .judged(judgment(Kind::Reply, 0.7, 0.8))
            .card(Texts {
                draft: "Bonjour Hugo,\n\nMerci pour votre candidature. Pour le poste senior nous cherchons un profil plus expérimenté, mais nous ouvrons un poste au printemps qui pourrait vous correspondre : puis-je garder votre dossier ?\n\nBien à vous,\nMaya Laurent".into(),
                ..texts(
                    "Répondre à Hugo",
                    "Tom et toi gardez son profil pour le printemps.",
                    "Hugo postule au poste de designer ; profil prometteur mais plus junior.",
                )
            })
            .chip(None, &["Hiring"]),
        conv("ines-icons", "Icon set v3: ready for review", &["ines"])
            .unread()
            .say("ines", Mins(5 * 60 + 10), "
Hi Maya,

Icon set v3 is ready: 48 icons, two weights, all on the 24px grid. I redrew the bike, the route and the battery icons following your notes. Preview attached.

If you're happy with it I'll export everything for Northbeam's developers tomorrow.

Inès")
            .file(png(
                "icons-v3-preview.png",
                art::banner([245, 247, 250], [228, 233, 240], [64, 120, 214], 7),
            ))
            .judged(judgment(Kind::Reply, 0.85, 1.6))
            .card(Texts {
                draft: "These look great, Inès, the battery icon is so much clearer now. Go ahead with the export!".into(),
                ..texts(
                    "Review Inès's icon set",
                    "She exports for Northbeam tomorrow.",
                    "Inès finished 48 icons in two weights and waits for your go-ahead.",
                )
            })
            .chip(None, &["Clients/Northbeam"]),
        conv("raj-estimate", "Estimate for the booking widget", &["raj"])
            .say("me", Day(5, 15, 30), "
Hi Raj,

We have a small build for Atelier Sève: a booking widget for their in-store workshops (calendar, payment, confirmation email). Designs are ready. Could you send us an estimate?

Maya")
            .say("raj", Day(4, 10, 2), "
Hi Maya, sounds fun. Let me look at the designs and I'll send a number by the end of the week. Do they already have a payment provider?

Raj")
            .say("me", Day(4, 10, 40), "Yes, they use the one built into their shop. Thanks Raj!")
            .status("waiting")
            .judged(judgment(Kind::Waiting, 0.15, 0.9))
            .card(texts(
                "Raj sends an estimate for the widget",
                "Promised by the end of the week.",
                "Raj is looking at the designs and will estimate the booking widget.",
            )),
        conv("offsite", "Studio offsite?", &["tom", "ines"])
            .say("tom", Day(19, 12, 4), "
Team, what about two days away together before the end-of-year rush? Somewhere reachable by train, a big table and good food. I'm thinking the Drôme.")
            .say("ines", Day(19, 12, 30), "Yes please!! I can look for a house.")
            .say("me", Day(19, 13, 2), "Love it. Any dates in the second half of next month work for me.")
            .say("ines", Day(17, 18, 45), "Found a lovely farmhouse near Dieulefit, 3 bedrooms, 290 € a night. Shall I book Thursday-Friday the 20th?")
            .say("tom", Day(17, 19, 10), "Book it!")
            .say("me", Day(17, 19, 22), "Yes! Thank you Inès 🙌")
            .status("done"),
        conv("monday-notes", "Notes from Monday's studio sync", &["tom", "ines"])
            .say("tom", Day(2, 18, 30), "
Quick notes from today:

- Northbeam: round 2 out, decision on navigation pending (Maya).
- Atelier Sève: quote approved verbally, waiting on paperwork.
- Hiring: Priya's interview to schedule (Maya).
- Studio: offsite booked; new printer arrives Thursday.

Next sync Monday 9:30.
T.")
            .status("done")
            .gist(
                "Monday sync: Northbeam navigation decision pending, Sève quote approved, Priya's interview to schedule, offsite booked.",
                "Your items: the Northbeam decision and Priya's interview.",
            ),
        conv("clara-tva", "TVA du trimestre : pièces manquantes", &["clara"])
            .folder("Finance")
            .starred()
            .unread()
            .say("clara", Mins(31 * 60), "
Bonjour Maya,

Je prépare la déclaration de TVA du trimestre. Il me manque trois justificatifs :

- la facture de l'écran (achat du 12 septembre) ;
- le ticket du déjeuner client du 3 octobre ;
- la facture de l'abonnement au logiciel de design.

Peux-tu me les envoyer d'ici vendredi ? La déclaration part lundi.

Merci !
Clara")
            .judged(Judgment {
                due: Some(date_in(now, 2)),
                ..judgment(Kind::Promise, 0.9, 2.4)
            })
            .card(Texts {
                points: vec![
                    "Facture de l'écran (12 septembre)".into(),
                    "Ticket du déjeuner client (3 octobre)".into(),
                    "Facture du logiciel de design".into(),
                ],
                draft: "Bonjour Clara,\n\nVoici les trois justificatifs en pièces jointes. Dis-moi s'il manque quoi que ce soit !\n\nMaya".into(),
                ..texts(
                    "Envoyer 3 justificatifs à Clara",
                    "La déclaration de TVA part lundi.",
                    "Clara demande trois justificatifs pour la TVA du trimestre, d'ici vendredi.",
                )
            })
            .gist(
                "Clara prépare la TVA du trimestre et attend trois justificatifs : l'écran, le déjeuner client du 3 octobre, l'abonnement au logiciel.",
                "Envoyer les trois pièces avant vendredi.",
            ),
    ]
}

fn home(now: i64) -> Vec<Conv> {
    vec![
        conv("maman", "Anniversaire de maman 🎂", &["elise"])
            .cc(&["julien"])
            .unread()
            .say("elise", Day(4, 20, 10), "
Coucou !

L'anniversaire de maman c'est dans deux semaines. J'ai pensé à un cours de céramique pour deux (elle en parle depuis des mois). Ça fait 135 € : on partage en trois avec Antoine ?")
            .say("me", Day(4, 21, 2), "Super idée !! Je suis partante. On lui offre au déjeuner du dimanche ?")
            .say("elise", Day(3, 8, 15), "Oui parfait. Je réserve aujourd'hui.")
            .say("elise", Mins(6 * 60 + 20), "
C'est réservé ! Tu peux me faire un virement de 45 € quand tu as une minute ? 😘

Et tu t'occupes du gâteau ?")
            .judged(Judgment {
                amount: Some("45 €".into()),
                ..judgment(Kind::Payment, 0.8, 1.5)
            })
            .card(Texts {
                draft: "C'est fait pour le virement ! Et oui, je m'occupe du gâteau 🍰".into(),
                ..texts(
                    "Virer 45 € à Élise",
                    "Le cadeau de maman est réservé.",
                    "Élise a réservé le cours de céramique (135 € à trois) et te demande ta part et le gâteau.",
                )
            }),
        conv("dinner", "Dinner Saturday?", &["julien"])
            .say("julien", Day(5, 12, 40), "Nadia and Sam are free Saturday. Dinner at ours? I'll cook the risotto.")
            .say("me", Day(5, 12, 52), "Yes! I'll get dessert and wine.")
            .say("julien", Day(5, 13, 5), "Deal. 8pm 😊")
            .say("me", Day(5, 13, 6), "❤️")
            .status("done"),
        conv("hike", "Photos from the hike", &["nadia"])
            .say("nadia", Day(2, 21, 44), "
What a day! Here are my two favourites. The view from the ridge was unreal.

Same time next month? 🥾")
            .file(png(
                "ridge.png",
                art::landscape([126, 177, 222], [76, 128, 82], [255, 214, 120]),
            ))
            .file(png(
                "lake.png",
                art::landscape([168, 206, 235], [52, 99, 120], [255, 236, 179]),
            ))
            .status("todo")
            .judged(judgment(Kind::Reply, 0.5, 0.4))
            .card(Texts {
                draft: "These are gorgeous, Nadia! Yes for next month, I'll bring the snacks this time 🥾".into(),
                ..texts(
                    "Reply to Nadia's photos",
                    "She suggests another hike next month.",
                    "Nadia sends two photos from the hike and suggests going again next month.",
                )
            })
            .chip(Some("done"), &[]),
        conv("plombier", "Travaux salle de bain : passage du plombier", &["gerard"])
            .folder("Home")
            .unread()
            .say("gerard", Mins(22 * 60), "
Bonjour Madame Laurent,

Le plombier passera après-demain à 9h pour la fuite sous le lavabo. Il en a pour une heure environ. Merci de me confirmer que quelqu'un sera présent pour lui ouvrir.

Cordialement,
Gérard Blanc
Gestion locative Blanc")
            .judged(Judgment {
                due: Some(date_in(now, 2)),
                due_time: Some("09:00".into()),
                ..judgment(Kind::Event, 0.8, 1.9)
            })
            .card(Texts {
                draft: "Bonjour Monsieur Blanc,\n\nC'est noté, je serai là à 9h pour lui ouvrir.\n\nCordialement,\nMaya Laurent".into(),
                ..texts(
                    "Confirmer le passage du plombier",
                    "Il vient après-demain à 9h.",
                    "Le plombier passe après-demain à 9h pour la fuite du lavabo ; il faut quelqu'un sur place.",
                )
            }),
        conv("sortie", "Sortie scolaire : autorisation à signer", &["ecole"])
            .unread()
            .say("ecole", Day(1, 16, 30), "
Madame, Monsieur,

La classe de CE2 se rendra au musée des Confluences jeudi prochain. Merci de retourner l'autorisation ci-jointe, signée, avant lundi. Prévoir un pique-nique.

Le secrétariat
École des Tilleuls")
            .file(pdf(
                "autorisation-sortie-confluences.pdf",
                "Autorisation de sortie scolaire",
                &[
                    "Classe : CE2",
                    "Sortie : musee des Confluences, jeudi",
                    "Depart 8h45, retour 16h00",
                    "Je soussigne(e) ........ autorise mon enfant a participer",
                    "Signature des parents :",
                ],
            ))
            .judged(Judgment {
                due: Some(date_in(now, 4)),
                ..judgment(Kind::Sign, 0.2, 1.7)
            })
            .card(texts(
                "Signer l'autorisation de sortie",
                "À rendre avant lundi.",
                "Sortie au musée des Confluences jeudi prochain : autorisation signée avant lundi, prévoir un pique-nique.",
            ))
            .chip(None, &["Home"]),
        conv("tiles", "Kitchen tiles: two options", &["bruno"])
            .folder("Home")
            .say("bruno", Day(3, 9, 5), "
Hello Maya,

As discussed, here are the two tiles we can get before the works start. The terracotta is warmer, the white zellige is easier to match with the worktop. Same price (64 €/m²).

I need your choice by Friday to order them.

Bruno
Ferreira Rénovation")
            .file(png(
                "terracotta.png",
                art::tiles([196, 110, 74], [214, 132, 92], [236, 224, 210]),
            ))
            .file(png(
                "zellige-blanc.png",
                art::tiles([244, 244, 238], [230, 233, 228], [196, 196, 190]),
            ))
            .status("todo")
            .judged(Judgment {
                due: Some(date_in(now, 3)),
                ..judgment(Kind::Decision, 0.9, 1.8)
            })
            .card(Texts {
                options: vec![
                    choice(
                        "Terracotta",
                        "Warmer, more character",
                        false,
                        "",
                        "Hi Bruno,\n\nLet's go with the terracotta. Thanks!\n\nMaya",
                    ),
                    choice(
                        "White zellige",
                        "Easier to match with the worktop",
                        true,
                        "It matches the oak worktop you already ordered.",
                        "Hi Bruno,\n\nLet's go with the white zellige. Thanks!\n\nMaya",
                    ),
                ],
                ..texts(
                    "Choose the kitchen tiles",
                    "Bruno orders on Friday.",
                    "Bruno offers terracotta or white zellige, same price; he needs an answer by Friday.",
                )
            }),
        conv("claim", "Votre dossier sinistre n°48213", &["horizon"])
            .folder("Home")
            .say("me", Day(12, 19, 0), "
Bonjour,

Suite à mon appel, je vous envoie les photos et la facture du réparateur pour le dégât des eaux du 2 octobre.

Cordialement,
Maya Laurent")
            .say("horizon", Day(10, 11, 20), "
Madame,

Nous accusons réception de votre dossier. Un expert vous contactera sous 15 jours ouvrés.

Le service sinistres
Assurance Horizon")
            .status("waiting")
            .judged(judgment(Kind::Waiting, 0.1, 0.5))
            .card(texts(
                "L'expert de l'assurance doit appeler",
                "Sous 15 jours ouvrés.",
                "Assurance Horizon a reçu le dossier du dégât des eaux ; un expert doit appeler.",
            )),
        conv("dentist", "Rappel : rendez-vous jeudi", &["cabinet"])
            .unread()
            .say("cabinet", Mins(3 * 60 + 5), "
Bonjour,

Nous vous rappelons votre rendez-vous de détartrage jeudi à 18h30 avec le Dr Morel.

En cas d'empêchement, merci de nous prévenir 24 h à l'avance.

Cabinet dentaire Morel")
            .judged(Judgment {
                due: Some(date_in(now, 3)),
                due_time: Some("18:30".into()),
                automated: Some(0.8),
                ..judgment(Kind::Event, 0.05, 1.2)
            })
            .card(texts(
                "Dentiste jeudi à 18h30",
                "Détartrage avec le Dr Morel.",
                "Rappel du rendez-vous de détartrage jeudi à 18h30.",
            )),
        conv("run", "Saturday long run?", &["sam"])
            .say("sam", Day(9, 7, 50), "Saturday 8am, quays to the Parc de la Tête d'Or, 15k easy pace?")
            .say("me", Day(9, 8, 20), "I'm in! Meet at the bridge?")
            .say("sam", Day(9, 8, 31), "Bridge it is 🏃‍♀️")
            .status("done"),
        conv("bookclub", "Book club: next pick", &["nadia", "sam"])
            .say("nadia", Day(15, 19, 30), "Three candidates for next month: a slow sci-fi novel, a memoir about lighthouses, or the Italian family saga. Vote!")
            .say("sam", Day(15, 20, 2), "Lighthouses 🗼")
            .say("me", Day(15, 21, 15), "Lighthouses too! I'll host this time.")
            .archived(),
    ]
}

/// Older mail, filed long ago, so the mailbox has a past.
fn history() -> Vec<Conv> {
    vec![
        conv("nb-analytics", "Access to the Northbeam analytics dashboard", &["marcus"])
            .folder("Clients/Northbeam")
            .say("marcus", Day(22, 9, 30), "Hi Maya, you and Tom now have read access to the analytics dashboard. The onboarding funnel is under Growth > Activation.")
            .say("me", Day(22, 10, 4), "Thanks Marcus, got in. The drop at bike pairing is even steeper than we thought!")
            .archived(),
        conv("riders", "Rider interviews: 8 confirmed", &["hana"])
            .cc(&["ellen"])
            .folder("Clients/Northbeam")
            .say("hana", Day(18, 16, 12), "
Hi Maya,

Eight riders confirmed for the interviews: five commuters, two sport riders and one delivery rider. They each get a voucher for the shop. I've put the schedule in the shared folder.

Hana")
            .say("me", Day(18, 17, 0), "Brilliant, thank you Hana! The delivery rider will be a great addition.")
            .archived(),
        conv("seve-shoot", "Photos du shooting", &["camille"])
            .folder("Clients/Sève")
            .say("camille", Day(30, 15, 45), "Voilà une première sélection du shooting de la gamme automne. Dis-moi lesquelles tu veux pour le site !")
            .file(png(
                "shooting-01.png",
                art::landscape([236, 214, 196], [176, 120, 90], [250, 240, 220]),
            ))
            .say("me", Day(29, 9, 20), "Elles sont magnifiques ! La première pour la page d'accueil, sans hésiter.")
            .archived(),
        conv("printer", "New printer arrives Thursday", &["tom"])
            .say("tom", Day(4, 17, 30), "FYI the new printer arrives Thursday morning. Can someone be at the studio before 10?")
            .say("me", Day(4, 17, 45), "I'll be there from 9.")
            .status("done"),
        conv("plants", "Studio plants 🌿", &["ines"])
            .say("ines", Day(64, 12, 0), "I'm going to the plant market on Saturday. Shall I get a big one for the corner by the window?")
            .say("me", Day(64, 12, 20), "Yes!! Something that survives us 😅")
            .archived(),
        conv("annecy", "Week-end à Annecy ?", &["elise", "julien"])
            .say("elise", Day(75, 19, 0), "On loue une maison à Annecy fin août avec les enfants ? Il y a de la place pour tout le monde.")
            .say("julien", Day(75, 19, 40), "Grave ! On peut amener les vélos.")
            .say("me", Day(75, 20, 5), "Oui oui oui. Je regarde les trains.")
            .archived(),
        conv("welcome-ines", "Welcome to the studio, Inès!", &["ines", "tom"])
            .say("me", Day(118, 9, 0), "
Inès, welcome to Fernhill! Your desk is the one by the window, your laptop is ready and the coffee machine is temperamental (Tom will explain).

So happy to have you with us.
Maya")
            .say("ines", Day(118, 9, 40), "Thank you both, I'm so excited to start! See you at 10 😊")
            .archived(),
        conv("logo-files", "Logo files for the website", &["raj"])
            .say("raj", Day(96, 14, 0), "Hi Maya, could you send the logo as SVG and the brand colours? Building the new studio site this week.")
            .say("me", Day(96, 15, 12), "Here you go! Colours are in the PDF.")
            .file(pdf(
                "Fernhill-brand.pdf",
                "Fernhill Studio - Brand basics",
                &[
                    "Ink #1F2A44",
                    "Fern #3E7C59",
                    "Sand #EADBC8",
                    "Type: a humanist sans for text, a serif for titles",
                ],
            ))
            .say("raj", Day(96, 15, 30), "Perfect, thanks!")
            .archived(),
        conv("students", "Portfolio reviews with the students", &["helene"])
            .say("helene", Day(52, 10, 15), "
Bonjour Maya,

Merci encore d'avoir accepté de faire les revues de portfolio avec nos étudiants de master. Les créneaux du mardi 14h-17h vous conviennent toujours ?

Hélène Vidal")
            .say("me", Day(52, 11, 2), "Bonjour Hélène, oui parfait, avec plaisir !")
            .say("helene", Day(44, 18, 20), "Les étudiants ont adoré, merci infiniment ! On recommence au printemps ?")
            .say("me", Day(44, 19, 0), "Avec grand plaisir 😊")
            .archived(),
        conv("lisbon-ideas", "Lisbon ideas", &["julien"])
            .say("julien", Day(40, 22, 10), "For the long weekend: Lisbon? Flights from Lyon are direct and the weather should still be lovely.")
            .say("me", Day(40, 22, 30), "Yes!! Let's book this weekend. I want to see the tiles museum.")
            .archived(),
        conv("taxes-2025", "Documents pour la liasse fiscale", &["clara"])
            .folder("Finance")
            .say("clara", Day(160, 9, 30), "Bonjour Maya, pourrais-tu m'envoyer les relevés bancaires de décembre et l'inventaire du matériel ? Merci !")
            .say("me", Day(159, 18, 0), "Voilà Clara, tout est en pièce jointe.")
            .file(pdf(
                "inventaire-materiel-2025.pdf",
                "Inventaire du materiel 2025",
                &[
                    "4 ordinateurs portables",
                    "2 ecrans 27 pouces",
                    "1 imprimante",
                    "Mobilier de bureau",
                ],
            ))
            .archived(),
    ]
}

/// The failed delivery of the prototype link (a mistyped address), threaded with it.
fn bounce(now: i64) -> Mail {
    let mut m = Mail::new(
        "nb-bounce",
        Address {
            name: "Mail Delivery Subsystem".into(),
            email: "mailer-daemon@fernhill.example".into(),
        },
        "Delivery Status Notification (Failure)",
        "
Your message wasn't delivered to marcos.lee@northbeam.example because the address couldn't be found, or is unable to receive mail.

The response from the remote server was:
550 5.1.1 The email account that you tried to reach does not exist."
            .trim()
            .to_string(),
        Mins(26 * 60 - 1).at(now),
    );
    m.unread = true;
    m.folders = vec!["Clients/Northbeam"];
    m.bounce = Some(crate::dsn::Dsn {
        recipient: "marcos.lee@northbeam.example".into(),
        status: "5.1.1".into(),
        diagnostic: "550 5.1.1 The email account that you tried to reach does not exist.".into(),
        original_message_id: None,
    });
    m
}
