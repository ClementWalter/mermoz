//! The demo's automated mail: newsletters, receipts and orders, deliveries, travel
//! bookings, calendar invitations and app notifications. Brands are made up and
//! generic; HTML mail shows pictures drawn by [`art`] and carried inside the message,
//! so nothing is ever loaded from the network.

use super::When::{Day, Mins};
use super::conversations::date_in;
use super::{Mail, Plan, When, art, automated_judgment, chip, me};
use crate::feed::{Judgment, Kind, Texts};
use crate::mime::ParsedAttachment;
use crate::model::Address;

fn sender(name: &str, email: &str) -> Address {
    Address {
        name: name.into(),
        email: email.into(),
    }
}

fn inline_png(cid: &str, data: Vec<u8>) -> ParsedAttachment {
    ParsedAttachment {
        filename: format!("{cid}.png"),
        mime: "image/png".into(),
        data,
        content_id: Some(cid.into()),
        inline: true,
    }
}

fn pdf(name: &str, title: &str, lines: &[String]) -> ParsedAttachment {
    ParsedAttachment {
        filename: name.into(),
        mime: "application/pdf".into(),
        data: art::pdf(title, lines),
        ..Default::default()
    }
}

fn hex(c: art::Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// A branded HTML email: a header picture, a title, paragraphs, an optional table of
/// (label, value) rows and a footer.
fn html(
    color: art::Rgb,
    cid: &str,
    title: &str,
    paras: &[String],
    rows: &[(String, String)],
    footer: &str,
) -> String {
    let mut body = String::new();
    for p in paras {
        body.push_str(&format!(
            "<p style=\"margin:0 0 14px;font-size:15px;line-height:1.55;color:#2b2f36\">{p}</p>"
        ));
    }
    if !rows.is_empty() {
        body.push_str("<table style=\"width:100%;border-collapse:collapse;margin:8px 0 18px\">");
        for (k, v) in rows {
            body.push_str(&format!(
                "<tr><td style=\"padding:8px 0;border-bottom:1px solid #e6e8ec;color:#5b6170;font-size:14px\">{k}</td>\
                 <td style=\"padding:8px 0;border-bottom:1px solid #e6e8ec;text-align:right;font-size:14px;color:#2b2f36\">{v}</td></tr>"
            ));
        }
        body.push_str("</table>");
    }
    format!(
        "<html><body style=\"margin:0;background:#f4f5f7;font-family:-apple-system,Helvetica,Arial,sans-serif\">\
<div style=\"max-width:600px;margin:0 auto;background:#ffffff\">\
<img src=\"cid:{cid}\" width=\"600\" alt=\"\" style=\"display:block;width:100%;height:auto\">\
<div style=\"padding:28px 32px\">\
<h1 style=\"margin:0 0 18px;font-size:24px;line-height:1.25;color:{}\">{title}</h1>{body}\
</div>\
<div style=\"padding:18px 32px;background:#f8f9fa;color:#8a909c;font-size:12px\">{footer}</div>\
</div></body></html>",
        hex(color)
    )
}

/// Turns HTML paragraphs back into the plain text part.
fn plain(title: &str, paras: &[String], rows: &[(String, String)]) -> String {
    let mut out = format!("{title}\n\n");
    for p in paras {
        out.push_str(&strip_tags(p));
        out.push_str("\n\n");
    }
    for (k, v) in rows {
        out.push_str(&format!("{k}: {}\n", strip_tags(v)));
    }
    out.trim_end().to_string()
}

fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out
}

pub(crate) fn build(now: i64, mails: &mut Vec<Mail>, plans: &mut Vec<Plan>) {
    newsletters(now, mails, plans);
    receipts(now, mails, plans);
    travel(now, mails, plans);
    invitations(now, mails, plans);
    notifications(now, mails, plans);
}

struct Letter {
    key: &'static str,
    name: &'static str,
    email: &'static str,
    list: &'static str,
    colors: (art::Rgb, art::Rgb, art::Rgb),
    /// Day of the week it arrives, as days back from the newest issue.
    offset: i64,
    hour: u32,
    issues: &'static [(&'static str, &'static str)],
    folder: Option<&'static str>,
    /// The user always archives it from the Feed, so it joins the reading batch.
    batched: bool,
}

const LETTERS: &[Letter] = &[
    Letter {
        key: "sketch",
        name: "The Product Sketch",
        email: "hello@productsketch.example",
        list: "weekly.productsketch.example",
        colors: ([238, 242, 255], [205, 214, 250], [92, 106, 214]),
        offset: 0,
        hour: 7,
        issues: &[
            (
                "Onboarding that respects people's time",
                "Three teams cut their sign-up flows in half without losing the details they needed.",
            ),
            (
                "The quiet power of empty states",
                "What your app says when there is nothing to show is the first thing new users read.",
            ),
            (
                "Designing for gloves, rain and sunlight",
                "Lessons from apps used outdoors: bigger targets, higher contrast, fewer gestures.",
            ),
            (
                "Critique without the tears",
                "A simple format for design reviews that keeps feedback about the work.",
            ),
            (
                "When to stop adding settings",
                "Every toggle is a decision you asked your users to make for you.",
            ),
            (
                "Prototypes that answer one question",
                "The fastest prototype is the one built to settle a single argument.",
            ),
            (
                "Writing microcopy with support tickets",
                "Your support inbox already knows which words confuse people.",
            ),
            (
                "The case for boring navigation",
                "Users don't remember clever menus. They remember getting lost.",
            ),
            (
                "Measuring delight, carefully",
                "Satisfaction surveys say less than what people do in their second week.",
            ),
            (
                "Hand-offs developers actually like",
                "Specs, states and edge cases: what to annotate and what to skip.",
            ),
            (
                "Small studios, big clients",
                "How independent teams keep large accounts without burning out.",
            ),
        ],
        folder: None,
        batched: true,
    },
    Letter {
        key: "ledger",
        name: "Morning Ledger",
        email: "briefing@morningledger.example",
        list: "daily.morningledger.example",
        colors: ([250, 246, 236], [236, 224, 196], [176, 120, 40]),
        offset: 1,
        hour: 6,
        issues: &[
            (
                "Freelancers and the new invoicing rules",
                "What changes for small businesses when e-invoicing becomes mandatory.",
            ),
            (
                "Why rents are slowing in mid-sized cities",
                "Three reasons, and what they mean for young households.",
            ),
            (
                "Bike makers bet on software",
                "Subscriptions, apps and services: the hardware is no longer the whole product.",
            ),
            (
                "The four-day week, two years later",
                "Companies that tried it share what stuck and what didn't.",
            ),
            (
                "Interest rates: what to watch this month",
                "A short guide for the rest of us.",
            ),
            (
                "Coworking spaces grow up",
                "From hot desks to studios with workshops and kitchens.",
            ),
            (
                "The return of the local bookshop",
                "How independent shops found their footing again.",
            ),
            (
                "Small exporters and the strong euro",
                "Who is hurt, who isn't, and why.",
            ),
            (
                "Rail travel is back in fashion",
                "Night trains, new lines and why business travellers are switching.",
            ),
            (
                "Hiring in a cooling market",
                "Fewer openings, more applicants per role, and longer processes.",
            ),
            (
                "Energy bills before winter",
                "What households can expect, and a few ways to prepare.",
            ),
        ],
        folder: None,
        batched: true,
    },
    Letter {
        key: "weekend",
        name: "Lyon ce week-end",
        email: "bonjour@lyonweekend.example",
        list: "news.lyonweekend.example",
        colors: ([255, 240, 232], [250, 205, 186], [214, 92, 64]),
        offset: 2,
        hour: 9,
        issues: &[
            (
                "Marché de créateurs à la Croix-Rousse",
                "Une centaine d'artisans, des ateliers pour enfants et un brunch en terrasse.",
            ),
            (
                "Les quais en fête",
                "Concerts gratuits, guinguettes et cinéma en plein air ce samedi.",
            ),
            (
                "Les meilleures boulangeries du 7e",
                "Notre tournée de dégustation, adresse par adresse.",
            ),
            (
                "Balade à vélo dans les Monts d'Or",
                "Un itinéraire de 35 km, avec une pause gourmande à mi-chemin.",
            ),
            (
                "Expo : la céramique contemporaine",
                "Une trentaine d'artistes réunis dans un ancien atelier de soierie.",
            ),
            (
                "Le festival des lumières approche",
                "Les dates, les nouveautés et nos conseils pour éviter la foule.",
            ),
            (
                "Brunchs du dimanche",
                "Cinq adresses où il fait bon traîner.",
            ),
            (
                "Puces du canal",
                "Ce qu'on y a trouvé, et ce qu'on aurait dû acheter.",
            ),
            (
                "Théâtre en famille",
                "Trois spectacles pour les 6-10 ans ce mois-ci.",
            ),
            (
                "Le retour des soirées jazz",
                "Les clubs rouvrent leurs caves pour l'automne.",
            ),
            (
                "Randonnée aux portes de la ville",
                "Trois boucles faciles accessibles en transports.",
            ),
        ],
        folder: None,
        batched: false,
    },
    Letter {
        key: "type",
        name: "Pixel & Type",
        email: "letters@pixelandtype.example",
        list: "pixelandtype.example",
        colors: ([240, 240, 236], [214, 214, 206], [40, 40, 44]),
        offset: 3,
        hour: 18,
        issues: &[
            (
                "Variable fonts in product UI",
                "One file, every weight: how teams use them in apps today.",
            ),
            (
                "Numbers that line up",
                "Tabular figures, and why your dashboard needs them.",
            ),
            (
                "Pairing a serif with a sans",
                "Five pairings that work, and why.",
            ),
            (
                "Line length on small screens",
                "The old rule of 66 characters, revisited for phones.",
            ),
            (
                "Icon and text, aligned",
                "Optical alignment tricks for buttons and lists.",
            ),
            (
                "The comeback of the grotesque",
                "Why so many brands chose the same kind of typeface.",
            ),
            (
                "Accessible contrast without the grey",
                "Readable interfaces that still feel soft.",
            ),
            (
                "Designing a type scale",
                "From a single ratio to a system people can use.",
            ),
            (
                "Hand lettering in digital brands",
                "When it adds warmth and when it gets in the way.",
            ),
            (
                "Kerning headlines by eye",
                "A practical guide for designers who are not type designers.",
            ),
            (
                "Fonts and file size",
                "Subsetting, formats and loading strategies.",
            ),
        ],
        folder: None,
        batched: false,
    },
    Letter {
        key: "trail",
        name: "Trail Notes",
        email: "notes@trailnotes.example",
        list: "trailnotes.example",
        colors: ([226, 240, 228], [180, 214, 186], [52, 120, 74]),
        offset: 4,
        hour: 12,
        issues: &[
            (
                "Autumn rides: what to pack",
                "Layers, lights and the snack that never fails.",
            ),
            (
                "Maintaining your chain in ten minutes",
                "Clean, lube, wipe: a routine for busy riders.",
            ),
            (
                "Five gravel loops near the Alps",
                "From an easy morning to a full-day adventure.",
            ),
            (
                "Riding in the rain without misery",
                "Fenders, tyres and the right attitude.",
            ),
            (
                "Bikepacking for beginners",
                "Your first overnight trip, step by step.",
            ),
            (
                "The case for flat pedals",
                "Comfort, confidence and fewer sore knees.",
            ),
            ("Night riding safely", "Lights, routes and being seen."),
            (
                "Training without a plan",
                "Ride more, ride happier: a relaxed approach.",
            ),
            (
                "Commuting by bike in winter",
                "Staying warm, dry and visible.",
            ),
            (
                "Picking your first e-bike",
                "Range, weight and what the numbers don't tell you.",
            ),
            (
                "Route planning apps compared",
                "What matters when you're actually on the bike.",
            ),
        ],
        folder: None,
        batched: true,
    },
    Letter {
        key: "kitchen",
        name: "Kitchen Table",
        email: "recipes@kitchentable.example",
        list: "kitchentable.example",
        colors: ([255, 246, 228], [250, 222, 170], [214, 140, 40]),
        offset: 5,
        hour: 17,
        issues: &[
            (
                "One-pan autumn dinners",
                "Squash, chickpeas and a sauce you'll make every week.",
            ),
            (
                "The perfect weeknight risotto",
                "Twenty-five minutes, one pot, no constant stirring.",
            ),
            (
                "Baking with apples",
                "A tart, a crumble and a cake for a lazy Sunday.",
            ),
            (
                "Soups for cold evenings",
                "Five recipes that freeze beautifully.",
            ),
            (
                "A better lunch box",
                "Ideas that survive a morning in a bag.",
            ),
            ("Bread at home, simply", "A no-knead loaf for beginners."),
            (
                "Cooking for friends without stress",
                "A menu you can prepare the day before.",
            ),
            (
                "Pasta from the pantry",
                "Six dinners from what you already have.",
            ),
            (
                "Weekend pancakes",
                "Fluffy, thin, savoury: pick your style.",
            ),
            (
                "Spices worth buying",
                "Five jars that change everyday cooking.",
            ),
            ("Desserts in a glass", "Pretty, easy, and made ahead."),
        ],
        folder: None,
        batched: true,
    },
];

fn newsletters(now: i64, mails: &mut Vec<Mail>, plans: &mut Vec<Plan>) {
    for (n, l) in LETTERS.iter().enumerate() {
        let (top, bottom, accent) = l.colors;
        for (i, (title, lede)) in l.issues.iter().enumerate() {
            // Weekly, newest first; the first issue arrived within the last week.
            let days = l.offset + 7 * i as i64;
            let key = format!("nl-{}-{i}", l.key);
            let cid = format!("{}-{i}", l.key);
            let paras = vec![
                format!("<strong>{lede}</strong>"),
                "This week we talked to people who do this every day, and kept the three ideas worth stealing. Grab a coffee: it's a five-minute read.".to_string(),
                "Also in this issue: a reader question, two links we loved, and next week's theme.".to_string(),
            ];
            let footer = format!(
                "You receive {} because you subscribed. To stop, reply with \"unsubscribe\".",
                l.name
            );
            let subject = title.to_string();
            let mut m = Mail::new(
                &key,
                sender(l.name, l.email),
                &subject,
                plain(title, &paras, &[]),
                Day(days, l.hour, 5 + n as u32).at(now),
            );
            m.html = Some(html(accent, &cid, title, &paras, &[], &footer));
            m.files = vec![inline_png(
                &cid,
                art::banner(top, bottom, accent, (n * 31 + i) as u64),
            )];
            m.list_id = Some(l.list.into());
            // Only the latest issue waits in the inbox; older ones were read and filed.
            m.inbox = i == 0;
            m.unread = i == 0;
            if let Some(f) = l.folder {
                m.folders = vec![f];
            }
            mails.push(m);
            if i == 0 {
                plans.push(Plan {
                    thread: key,
                    judgment: Some(automated_judgment(Kind::Reading)),
                    chip: (!l.batched).then(|| chip(Some("archive"), &[])),
                    card: Some(Texts {
                        summary: lede.to_string(),
                        ..Default::default()
                    }),
                    ..Default::default()
                });
            } else if l.batched && i <= 3 {
                plans.push(Plan {
                    thread: key,
                    archived_from_feed: true,
                    ..Default::default()
                });
            }
        }
    }
}

/// A receipt, order or bill from a shop or a utility.
struct Receipt {
    key: &'static str,
    brand: &'static str,
    email: &'static str,
    subject: &'static str,
    amount: &'static str,
    days: i64,
    color: art::Rgb,
    items: &'static [(&'static str, &'static str)],
    inbox: bool,
    folder: Option<&'static str>,
    invoice: bool,
}

const RECEIPTS: &[Receipt] = &[
    Receipt {
        key: "coffee-oct",
        brand: "Brewline Coffee",
        email: "orders@brewline.example",
        subject: "Your Brewline subscription has shipped",
        amount: "€24.00",
        days: 1,
        color: [120, 78, 52],
        items: &[
            ("Ethiopia, washed, 500 g", "€14.00"),
            ("Colombia, honey, 250 g", "€10.00"),
        ],
        inbox: true,
        folder: None,
        invoice: false,
    },
    Receipt {
        key: "cloudnest",
        brand: "Cloudnest",
        email: "billing@cloudnest.example",
        subject: "Receipt for your Cloudnest Team plan",
        amount: "€36.00",
        days: 3,
        color: [58, 120, 214],
        items: &[
            ("Team plan, 4 seats, monthly", "€30.00"),
            ("VAT 20%", "€6.00"),
        ],
        inbox: true,
        folder: Some("Finance"),
        invoice: true,
    },
    Receipt {
        key: "volta",
        brand: "Volta Énergie",
        email: "factures@volta-energie.example",
        subject: "Votre facture d'électricité est disponible",
        amount: "68,40 €",
        days: 2,
        color: [244, 180, 40],
        items: &[
            ("Consommation septembre", "52,10 €"),
            ("Abonnement", "16,30 €"),
        ],
        inbox: true,
        folder: Some("Home"),
        invoice: true,
    },
    Receipt {
        key: "mobilo",
        brand: "Mobilo",
        email: "facture@mobilo.example",
        subject: "Votre facture mobile d'octobre",
        amount: "19,99 €",
        days: 6,
        color: [214, 64, 120],
        items: &[("Forfait 100 Go", "19,99 €")],
        inbox: false,
        folder: Some("Home"),
        invoice: true,
    },
    Receipt {
        key: "papeterie",
        brand: "Papeterie Lumière",
        email: "commandes@papeterie-lumiere.example",
        subject: "Merci pour votre commande n°20931",
        amount: "42,50 €",
        days: 4,
        color: [64, 96, 160],
        items: &[
            ("Carnets pointillés A5 x3", "27,00 €"),
            ("Feutres fins x10", "15,50 €"),
        ],
        inbox: true,
        folder: None,
        invoice: false,
    },
    Receipt {
        key: "books",
        brand: "Bookshelf & Co.",
        email: "orders@bookshelfco.example",
        subject: "Order confirmed: 2 books",
        amount: "€38.90",
        days: 9,
        color: [150, 60, 60],
        items: &[
            ("The Lighthouse Keepers (paperback)", "€16.90"),
            ("Designing Calm Interfaces", "€22.00"),
        ],
        inbox: false,
        folder: None,
        invoice: false,
    },
    Receipt {
        key: "basket-1",
        brand: "Green Basket",
        email: "hello@greenbasket.example",
        subject: "Your vegetable box is on its way",
        amount: "€29.00",
        days: 5,
        color: [76, 160, 90],
        items: &[
            ("Seasonal box, medium", "€25.00"),
            ("Free-range eggs x6", "€4.00"),
        ],
        inbox: false,
        folder: None,
        invoice: false,
    },
    Receipt {
        key: "basket-2",
        brand: "Green Basket",
        email: "hello@greenbasket.example",
        subject: "Your vegetable box is on its way",
        amount: "€29.00",
        days: 12,
        color: [76, 160, 90],
        items: &[
            ("Seasonal box, medium", "€25.00"),
            ("Sourdough loaf", "€4.00"),
        ],
        inbox: false,
        folder: None,
        invoice: false,
    },
    Receipt {
        key: "frameworks",
        brand: "Frameworks Pro",
        email: "billing@frameworkspro.example",
        subject: "Your Frameworks Pro invoice",
        amount: "€54.00",
        days: 14,
        color: [40, 40, 44],
        items: &[
            ("Professional plan, 3 editors", "€45.00"),
            ("VAT 20%", "€9.00"),
        ],
        inbox: false,
        folder: Some("Finance"),
        invoice: true,
    },
    Receipt {
        key: "monitor",
        brand: "Écran Direct",
        email: "commandes@ecran-direct.example",
        subject: "Facture de votre commande n°77812",
        amount: "389,00 €",
        days: 32,
        color: [30, 120, 160],
        items: &[("Écran 27 pouces 4K", "389,00 €")],
        inbox: false,
        folder: Some("Finance"),
        invoice: true,
    },
    Receipt {
        key: "coffee-sep",
        brand: "Brewline Coffee",
        email: "orders@brewline.example",
        subject: "Your Brewline subscription has shipped",
        amount: "€24.00",
        days: 31,
        color: [120, 78, 52],
        items: &[
            ("Kenya, washed, 500 g", "€14.00"),
            ("Brazil, natural, 250 g", "€10.00"),
        ],
        inbox: false,
        folder: None,
        invoice: false,
    },
    Receipt {
        key: "cloudnest-sep",
        brand: "Cloudnest",
        email: "billing@cloudnest.example",
        subject: "Receipt for your Cloudnest Team plan",
        amount: "€36.00",
        days: 33,
        color: [58, 120, 214],
        items: &[
            ("Team plan, 4 seats, monthly", "€30.00"),
            ("VAT 20%", "€6.00"),
        ],
        inbox: false,
        folder: Some("Finance"),
        invoice: true,
    },
    Receipt {
        key: "volta-sep",
        brand: "Volta Énergie",
        email: "factures@volta-energie.example",
        subject: "Votre facture d'électricité est disponible",
        amount: "71,15 €",
        days: 32,
        color: [244, 180, 40],
        items: &[("Consommation août", "54,85 €"), ("Abonnement", "16,30 €")],
        inbox: false,
        folder: Some("Home"),
        invoice: true,
    },
    Receipt {
        key: "cinema",
        brand: "Ciné Lumière",
        email: "billets@cine-lumiere.example",
        subject: "Vos billets pour samedi 20h",
        amount: "21,00 €",
        days: 16,
        color: [120, 40, 140],
        items: &[("2 places plein tarif", "21,00 €")],
        inbox: false,
        folder: None,
        invoice: false,
    },
    Receipt {
        key: "plants",
        brand: "Leafy",
        email: "orders@leafy.example",
        subject: "Your plants are ready for pickup",
        amount: "€46.00",
        days: 22,
        color: [60, 140, 100],
        items: &[("Monstera, 60 cm", "€34.00"), ("Terracotta pot", "€12.00")],
        inbox: false,
        folder: None,
        invoice: false,
    },
    Receipt {
        key: "train-pass",
        brand: "TCL Mobilités",
        email: "noreply@tcl-mobilites.example",
        subject: "Votre abonnement mensuel est renouvelé",
        amount: "65,00 €",
        days: 13,
        color: [200, 30, 50],
        items: &[("Abonnement Tout Lyon, octobre", "65,00 €")],
        inbox: false,
        folder: None,
        invoice: false,
    },
];

fn receipts(now: i64, mails: &mut Vec<Mail>, plans: &mut Vec<Plan>) {
    for (n, r) in RECEIPTS.iter().enumerate() {
        let key = format!("rc-{}", r.key);
        let cid = format!("logo-{}", r.key);
        let mut rows: Vec<(String, String)> = r
            .items
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        rows.push(("Total".into(), format!("<strong>{}</strong>", r.amount)));
        let french = r.subject.starts_with("Vo")
            || r.subject.starts_with("Merci")
            || r.subject.starts_with("Fact");
        let (para, footer) = if french {
            (
                "Merci pour votre confiance. Voici le récapitulatif de votre achat.",
                "Ce message vous est envoyé automatiquement, merci de ne pas y répondre.",
            )
        } else {
            (
                "Thanks for your order! Here is your summary.",
                "This is an automated message, please do not reply.",
            )
        };
        let paras = vec![para.to_string()];
        let mut m = Mail::new(
            &key,
            sender(r.brand, r.email),
            r.subject,
            plain(r.subject, &paras, &rows),
            Day(r.days, 8 + (n as u32 % 10), 12 + n as u32).at(now),
        );
        m.html = Some(html(r.color, &cid, r.brand, &paras, &rows, footer));
        m.files = vec![inline_png(
            &cid,
            art::banner([250, 250, 250], [236, 238, 241], r.color, n as u64 + 100),
        )];
        if r.invoice {
            let lines: Vec<String> = r
                .items
                .iter()
                .map(|(k, v)| format!("{k} ........ {v}"))
                .chain([format!("Total ........ {}", r.amount)])
                .collect();
            m.files.push(pdf(
                &format!("{}-invoice.pdf", r.key),
                &format!("{} - invoice", r.brand),
                &lines,
            ));
        }
        m.inbox = r.inbox;
        m.unread = r.inbox && r.days <= 2;
        if let Some(f) = r.folder {
            m.folders = vec![f];
        }
        mails.push(m);
        if r.inbox {
            // The electricity bill is the one receipt still to pay.
            let judgment = if r.key == "volta" {
                Judgment {
                    amount: Some(r.amount.into()),
                    due: Some(date_in(now, 6)),
                    ..automated_judgment(Kind::Payment)
                }
            } else {
                automated_judgment(Kind::Cleanup)
            };
            let card = (r.key == "volta").then(|| Texts {
                title: "Payer la facture d'électricité".into(),
                why: "Prélèvement dans six jours.".into(),
                summary: "Facture Volta Énergie de 68,40 € pour septembre.".into(),
                ..Default::default()
            });
            plans.push(Plan {
                thread: key,
                judgment: Some(judgment),
                card,
                chip: r
                    .folder
                    .filter(|_| r.key != "volta")
                    .map(|f| chip(Some("archive"), &[f])),
                ..Default::default()
            });
        }
    }
    // Deliveries for the orders above.
    let parcels: &[(&str, &str, i64, &str)] = &[
        (
            "brewline",
            "Your parcel from Brewline Coffee arrives tomorrow",
            0,
            "Arriving tomorrow between 9am and 1pm.",
        ),
        (
            "papeterie",
            "Votre colis Papeterie Lumière a été livré",
            2,
            "Votre colis a été déposé dans votre boîte aux lettres.",
        ),
        (
            "books",
            "Your parcel is out for delivery",
            8,
            "Your parcel will be delivered today.",
        ),
        (
            "plants",
            "Your parcel has been delivered",
            21,
            "Delivered to the parcel locker on rue Vendôme.",
        ),
        (
            "monitor",
            "Votre colis est en route",
            30,
            "Livraison prévue demain avant 18h.",
        ),
        (
            "monitor-2",
            "Votre colis a été livré",
            29,
            "Remis en main propre.",
        ),
    ];
    for (i, (key, subject, days, line)) in parcels.iter().enumerate() {
        let cid = format!("parcelo-{i}");
        let tracking = format!("PC{:08}FR", 41_207_330 + i * 7_919);
        let rows = vec![("Tracking number".to_string(), tracking.clone())];
        let paras = vec![line.to_string()];
        let mut m = Mail::new(
            &format!("ship-{key}"),
            sender("Parcelo", "tracking@parcelo.example"),
            subject,
            plain(subject, &paras, &rows),
            if *days == 0 {
                Mins(95).at(now)
            } else {
                Day(*days, 11, 40).at(now)
            },
        );
        m.html = Some(html(
            [232, 120, 40],
            &cid,
            subject,
            &paras,
            &rows,
            "Parcelo · automated tracking update",
        ));
        m.files = vec![inline_png(
            &cid,
            art::banner(
                [255, 244, 232],
                [252, 214, 178],
                [232, 120, 40],
                300 + i as u64,
            ),
        )];
        m.inbox = *days <= 2;
        m.unread = *days == 0;
        mails.push(m);
        plans.push(Plan {
            thread: format!("ship-{key}"),
            judgment: (*days <= 2).then(|| automated_judgment(Kind::Cleanup)),
            archived_from_feed: *days > 2,
            ..Default::default()
        });
    }
}

fn local_day(now: i64, days: i64, hour: u32, minute: u32) -> chrono::DateTime<chrono::Local> {
    let base = chrono::DateTime::from_timestamp_millis(now)
        .unwrap_or_default()
        .with_timezone(&chrono::Local)
        .date_naive()
        + chrono::Duration::days(days);
    base.and_hms_opt(hour, minute, 0)
        .and_then(|t| t.and_local_timezone(chrono::Local).earliest())
        .unwrap_or_else(chrono::Local::now)
}

/// (key, brand, address, subject, booked days ago, color, details, departure when upcoming)
type Trip = (
    &'static str,
    &'static str,
    &'static str,
    String,
    i64,
    art::Rgb,
    Vec<(String, String)>,
    Option<chrono::DateTime<chrono::Local>>,
);

/// (key, organizer key, organizer, title, in days, hour, minutes, place, address)
type Invite = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    i64,
    u32,
    i64,
    &'static str,
    &'static str,
);

fn travel(now: i64, mails: &mut Vec<Mail>, plans: &mut Vec<Plan>) {
    let paris = local_day(now, 7, 7, 4);
    let paris_back = local_day(now, 7, 19, 2);
    let offsite = local_day(now, 14, 9, 15);
    let lisbon = local_day(now, -20, 10, 25);
    let trips: Vec<Trip> = vec![
        (
            "rail-paris",
            "Atlas Rail",
            "tickets@atlasrail.example",
            format!(
                "Your e-tickets: Lyon → Paris, {}",
                paris.format("%A %-d %B")
            ),
            6,
            [140, 30, 60],
            vec![
                (
                    "Outbound".into(),
                    format!(
                        "{} Lyon Part-Dieu → Paris Gare de Lyon",
                        paris.format("%a %-d %b, %H:%M")
                    ),
                ),
                (
                    "Return".into(),
                    format!(
                        "{} Paris Gare de Lyon → Lyon Part-Dieu",
                        paris_back.format("%a %-d %b, %H:%M")
                    ),
                ),
                ("Coach / seat".into(), "Coach 14, seat 62 (window)".into()),
                ("Booking reference".into(), "QXR7LM".into()),
                ("Total".into(), "<strong>€118.00</strong>".into()),
            ],
            Some(paris),
        ),
        (
            "rail-offsite",
            "Atlas Rail",
            "tickets@atlasrail.example",
            format!(
                "Your e-tickets: Lyon → Valence, {}",
                offsite.format("%A %-d %B")
            ),
            15,
            [140, 30, 60],
            vec![
                (
                    "Outbound".into(),
                    format!(
                        "{} Lyon Part-Dieu → Valence Ville",
                        offsite.format("%a %-d %b, %H:%M")
                    ),
                ),
                ("Passengers".into(), "3 adults".into()),
                ("Booking reference".into(), "HT2W9P".into()),
                ("Total".into(), "<strong>€87.60</strong>".into()),
            ],
            Some(offsite),
        ),
        (
            "car-offsite",
            "Roadly",
            "bookings@roadly.example",
            "Your car is booked: Valence station".to_string(),
            15,
            [30, 150, 140],
            vec![
                (
                    "Pick-up".into(),
                    format!(
                        "{} at Valence Ville station",
                        offsite.format("%a %-d %b, 10:30")
                    ),
                ),
                ("Car".into(), "Compact estate, automatic".into()),
                ("Total".into(), "<strong>€134.00</strong> for 2 days".into()),
            ],
            None,
        ),
        (
            "flight-lisbon",
            "Blue Heron Airways",
            "booking@blueheron-air.example",
            "Booking confirmed: Lyon → Lisbon".to_string(),
            38,
            [24, 74, 140],
            vec![
                (
                    "Flight".into(),
                    format!(
                        "BH 412, {} Lyon → Lisbon",
                        lisbon.format("%a %-d %b, %H:%M")
                    ),
                ),
                ("Return".into(), "BH 413, 3 days later, 18:40".into()),
                ("Passengers".into(), "Maya Laurent, Julien Laurent".into()),
                ("Booking reference".into(), "BH9KQ2".into()),
                ("Total".into(), "<strong>€246.00</strong>".into()),
            ],
            None,
        ),
        (
            "hotel-lisbon",
            "Casa Azulejo",
            "reservas@casa-azulejo.example",
            "Your stay in Lisbon is confirmed".to_string(),
            37,
            [40, 110, 170],
            vec![
                (
                    "Check-in".into(),
                    format!("{}, from 3pm", lisbon.format("%a %-d %b")),
                ),
                ("Nights".into(), "3".into()),
                ("Room".into(), "Double room with balcony, Alfama".into()),
                (
                    "Total".into(),
                    "<strong>€372.00</strong>, paid at the hotel".into(),
                ),
            ],
            None,
        ),
        (
            "checkin-lisbon",
            "Blue Heron Airways",
            "checkin@blueheron-air.example",
            "Check-in is open for your flight to Lisbon".to_string(),
            21,
            [24, 74, 140],
            vec![
                ("Flight".into(), "BH 412 Lyon → Lisbon".into()),
                ("Boarding passes".into(), "attached".into()),
            ],
            None,
        ),
    ];
    for (i, (key, brand, email, subject, booked, color, rows, upcoming)) in
        trips.into_iter().enumerate()
    {
        let cid = format!("travel-{key}");
        let paras = vec![if brand == "Casa Azulejo" {
            "Olá Maya! We look forward to welcoming you in Lisbon.".to_string()
        } else {
            "Thank you for travelling with us. Your booking details are below; your tickets are attached.".to_string()
        }];
        let mut m = Mail::new(
            &format!("tr-{key}"),
            sender(brand, email),
            &subject,
            plain(&subject, &paras, &rows),
            Day(booked, 20, 10 + i as u32).at(now),
        );
        m.html = Some(html(
            color,
            &cid,
            &subject,
            &paras,
            &rows,
            &format!("{brand} · booking confirmation"),
        ));
        m.files = vec![inline_png(
            &cid,
            art::banner([236, 244, 252], [196, 220, 244], color, 500 + i as u64),
        )];
        if brand != "Casa Azulejo" {
            let lines: Vec<String> = rows
                .iter()
                .map(|(k, v)| format!("{k}: {}", strip_tags(v)))
                .collect();
            m.files.push(pdf(
                &format!("{key}-tickets.pdf"),
                &format!("{brand} - tickets"),
                &lines,
            ));
        }
        m.folders = vec!["Travel"];
        m.inbox = upcoming.is_some();
        mails.push(m);
        if let Some(at) = upcoming {
            let days = (at.date_naive()
                - chrono::DateTime::from_timestamp_millis(now)
                    .unwrap_or_default()
                    .with_timezone(&chrono::Local)
                    .date_naive())
            .num_days();
            plans.push(Plan {
                thread: format!("tr-{key}"),
                status: Some("backlog"),
                judgment: Some(Judgment {
                    due: Some(date_in(now, days)),
                    due_time: Some(at.format("%H:%M").to_string()),
                    ..automated_judgment(Kind::Event)
                }),
                card: Some(Texts {
                    title: format!(
                        "Train to {}",
                        if key == "rail-paris" {
                            "Paris"
                        } else {
                            "Valence"
                        }
                    ),
                    why: format!("Leaves {} at {}.", at.format("%A"), at.format("%H:%M")),
                    summary: format!(
                        "{} from Lyon Part-Dieu; tickets attached.",
                        at.format("%A %-d %B")
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }
    }
}

fn invitations(now: i64, mails: &mut Vec<Mail>, plans: &mut Vec<Plan>) {
    let invites: &[Invite] = &[
        (
            "inv-rider-test",
            "ellen",
            "Ellen Shaw",
            "Northbeam rider test",
            9,
            14,
            120,
            "Northbeam HQ, Villeurbanne",
            "ellen.shaw@northbeam.example",
        ),
        (
            "inv-sync",
            "tom",
            "Tom Becker",
            "Weekly studio sync",
            5,
            9,
            30,
            "Studio, big table",
            "tom@fernhill.example",
        ),
        (
            "inv-seve",
            "camille",
            "Camille Roux",
            "Atelier Sève : revue des emballages",
            11,
            10,
            60,
            "Atelier Sève, Lyon 2e",
            "camille@atelier-seve.example",
        ),
        (
            "inv-crit",
            "ines",
            "Inès Moreau",
            "Design crit: icon set and onboarding",
            2,
            16,
            45,
            "Studio",
            "ines@fernhill.example",
        ),
        (
            "inv-clara",
            "clara",
            "Clara Dupont",
            "Point bilan semestriel",
            16,
            11,
            60,
            "Visio",
            "clara@dupont-compta.example",
        ),
        (
            "inv-dinner",
            "elise",
            "Élise Laurent",
            "Déjeuner d'anniversaire de maman",
            13,
            12,
            180,
            "Chez Élise",
            "elise.laurent@mail.example",
        ),
    ];
    for (i, (key, _who, name, title, in_days, hour, minutes, place, email)) in
        invites.iter().enumerate()
    {
        let start = local_day(now, *in_days, *hour, 0);
        let when = start.format("%A %-d %B, %H:%M").to_string();
        let text = format!(
            "{name} invited you to: {title}\n\nWhen: {when} ({minutes} min)\nWhere: {place}\n\nAccept or decline in your calendar."
        );
        let mut m = Mail::new(
            key,
            sender(name, email),
            &format!("Invitation: {title} @ {when}"),
            text,
            Day(1 + (i as i64 % 4), 9 + i as u32, 3).at(now),
        );
        m.to = vec![me()];
        m.files = vec![ParsedAttachment {
            filename: "invite.ics".into(),
            mime: "text/calendar".into(),
            data: art::ics(
                &format!("{key}@fernhill.example"),
                title,
                start.with_timezone(&chrono::Utc),
                *minutes,
                email,
                place,
            ),
            ..Default::default()
        }];
        m.unread = i % 2 == 0;
        mails.push(m);
        plans.push(Plan {
            thread: key.to_string(),
            judgment: Some(Judgment {
                due: Some(date_in(now, *in_days)),
                due_time: Some(format!("{hour:02}:00")),
                ..automated_judgment(Kind::Event)
            }),
            card: Some(Texts {
                title: title.to_string(),
                why: format!("{when}, {place}."),
                summary: format!("{name} invites you: {title}."),
                ..Default::default()
            }),
            ..Default::default()
        });
    }
}

fn notifications(now: i64, mails: &mut Vec<Mail>, plans: &mut Vec<Plan>) {
    // (key, app, address, subject, body, minutes ago or days ago, inbox)
    let items: &[(&str, &str, &str, &str, &str, When, bool)] = &[
        (
            "canvas-1",
            "Canvasly",
            "notifications@canvasly.example",
            "Ellen Shaw commented on “Ride tracking v2”",
            "Ellen Shaw: “Can the pause button be a bit bigger? Gloves!”",
            Mins(150),
            true,
        ),
        (
            "canvas-2",
            "Canvasly",
            "notifications@canvasly.example",
            "Marcus Lee commented on “Ride tracking v2”",
            "Marcus Lee: “Option A feels right on my phone.”",
            Day(2, 9, 50),
            true,
        ),
        (
            "canvas-3",
            "Canvasly",
            "notifications@canvasly.example",
            "Inès Moreau shared “Icon set v3” with you",
            "Inès Moreau invited you to edit “Icon set v3”.",
            Mins(300),
            true,
        ),
        (
            "canvas-4",
            "Canvasly",
            "notifications@canvasly.example",
            "3 new comments on “Sève packaging”",
            "Camille Roux and 1 other left comments.",
            Day(6, 17, 4),
            false,
        ),
        (
            "bank-1",
            "Banque Rhône",
            "alertes@banque-rhone.example",
            "Virement reçu : 2 400,00 €",
            "Un virement de ATELIER SEVE de 2 400,00 € a été crédité sur votre compte professionnel.",
            Day(1, 10, 20),
            true,
        ),
        (
            "bank-2",
            "Banque Rhône",
            "alertes@banque-rhone.example",
            "Votre relevé de septembre est disponible",
            "Votre relevé de compte est disponible dans votre espace client.",
            Day(12, 7, 0),
            false,
        ),
        (
            "bank-3",
            "Banque Rhône",
            "alertes@banque-rhone.example",
            "Paiement par carte : 389,00 €",
            "Paiement de 389,00 € chez ECRAN DIRECT.",
            Day(32, 15, 2),
            false,
        ),
        (
            "cloud-1",
            "Cloudnest",
            "no-reply@cloudnest.example",
            "Tom Becker shared “Northbeam / Research” with you",
            "You can now view and edit 24 files.",
            Day(20, 11, 0),
            false,
        ),
        (
            "cloud-2",
            "Cloudnest",
            "no-reply@cloudnest.example",
            "Your storage is 80% full",
            "Your team has used 80 GB of 100 GB.",
            Day(4, 6, 30),
            true,
        ),
        (
            "tasks-1",
            "Taskboard",
            "digest@taskboard.example",
            "Your weekly digest: 12 tasks done",
            "12 tasks done, 5 due this week, 2 overdue.",
            Day(3, 8, 0),
            true,
        ),
        (
            "tasks-2",
            "Taskboard",
            "digest@taskboard.example",
            "Your weekly digest: 9 tasks done",
            "9 tasks done, 7 due this week.",
            Day(10, 8, 0),
            false,
        ),
        (
            "cowork-1",
            "La Fabrique",
            "hello@lafabrique-coworking.example",
            "Salle Rhône réservée mardi 14h-16h",
            "Votre réservation de la salle Rhône est confirmée.",
            Day(2, 13, 15),
            true,
        ),
        (
            "cowork-2",
            "La Fabrique",
            "hello@lafabrique-coworking.example",
            "Le programme des ateliers de novembre",
            "Ateliers, apéros et petits-déjeuners : le programme du mois.",
            Day(9, 10, 0),
            false,
        ),
        (
            "security",
            "Cloudnest",
            "security@cloudnest.example",
            "New sign-in on a Mac",
            "We noticed a new sign-in to your account from Safari on macOS, Lyon.",
            Day(7, 22, 41),
            false,
        ),
    ];
    for (key, app, email, subject, body, when, inbox) in items.iter() {
        let mut m = Mail::new(
            &format!("nt-{key}"),
            sender(app, email),
            subject,
            format!("{body}\n\n— {app}"),
            when.at(now),
        );
        m.inbox = *inbox;
        m.unread = *inbox && matches!(*key, "canvas-1" | "bank-1" | "canvas-3");
        if key.starts_with("bank") {
            m.folders = vec!["Finance"];
        }
        mails.push(m);
        if *inbox {
            plans.push(Plan {
                thread: format!("nt-{key}"),
                judgment: Some(automated_judgment(Kind::Cleanup)),
                chip: key
                    .starts_with("canvas")
                    .then(|| chip(Some("archive"), &[])),
                ..Default::default()
            });
        }
    }
}
