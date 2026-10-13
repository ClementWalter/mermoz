//! Regression fixtures for the reading pipeline. Every fixture is fictional (names,
//! addresses, content) and keeps the structure of a real failure class: how a client
//! quotes, forwards, signs, encodes or decorates mail.

use super::*;
use rstest::rstest;

/// A plain-text message with a subject.
fn mail<'a>(subject: &'a str, text: &'a str) -> Mail<'a> {
    Mail {
        subject,
        text,
        ..Default::default()
    }
}

// ---- Replies: each client's attribution and quote ----

const GMAIL_FR_NBSP: &str = "Parfait, à demain.\r\n\r\nLe lun. 12 janv. 2026 à 11:54, Inès Morel <ines@poste.example> a écrit\u{a0}:\r\n\r\n> On se voit à 9h ?\r\n";
const GMAIL_FR_WRAPPED: &str = "Ça marche.\n\nLe lun. 12 janv. 2026 à 11:54, Inès Morel <\nines@poste.example> a écrit :\n\n> On se voit à 9h ?\n";
const GMAIL_EN_WRAPPED: &str = "Sounds good.\n\nOn Sun, Oct 11, 2026 at 8:12 AM Marc Petit <marc@mail.example>\nwrote:\n\n> Lunch on Friday?\n";
const OUTLOOK_EN: &str = "Thanks a lot!\r\n\r\n________________________________\r\nFrom: Hana Ortiz <hana@corp.example>\r\nSent: Monday, 29 June 2026 11:07\r\nTo: Teo Blanc <teo@firm.example>\r\nSubject: Re: Budget\r\n\r\nHi Teo,\r\nSee the figures.\r\n";
const OUTLOOK_FR: &str = "Merci pour le retour.\n\nDe : Hana Ortiz <hana@corp.example>\nEnvoyé : lundi 29 juin 2026 11:07\nÀ : Teo Blanc <teo@firm.example>\nObjet : RE: Budget\n\nBonjour Teo,\n";
const ORIGINAL_MESSAGE: &str = "Noted, I will call them.\n\n-----Original Message-----\nFrom: Desk <desk@shop.example>\nSent: Tuesday\nTo: Teo\nSubject: Order\n\nYour order shipped.\n";
const ZIMBRA: &str = "Je confirme ma présence.\n\n----- Mail original -----\nDe: Club <club@assoc.example>\nÀ: Teo\nEnvoyé: Mardi\nObjet: AG\n\nL'AG aura lieu jeudi.\n";
const SAMSUNG_GLUED: &str = "Bien noté !-------- Message d'origine --------De : Lou Petit <lou@mail.example> Date : 05/01/2026 À : Teo Objet : Vélos\n\nLes vélos sont réparés.\n";
const ITALIAN: &str = "Volentieri!\n\nIl giorno 14 apr 2026, alle ore 20:18, Lea Conti <lea@posta.example> ha scritto:\n\n> Cena sabato?\n";
const HORDE: &str =
    "Great, count me in.\n\nQuoting Sam Ito <sam@uni.example>:\n\n> Anyone for the hike?\n";
const BARE_ATTRIBUTION: &str =
    "D'accord pour mardi.\n\nLou Petit a écrit :\n> Mardi ou mercredi ?\n";
const TRAILING_QUOTE: &str = "Yes.\n\n> Are you coming?\n> Let me know.\n";
const BOTTOM_POSTED: &str = "Le 3 mai 2026 à 10:00, Lou Petit <lou@mail.example> a écrit :\n\n> Tu as les clés ?\n\nOui, je les ai.\n";
const INLINE_REPLY: &str =
    "> Can you bring the cables?\nYes, both of them.\n> And the projector?\nNo, it is broken.\n";
const ORANGE_QUOTED_HEADER: &str = "Pas libre ce week-end.\n\n> Message du 25/01/26 17:59\n> De : sam@mail.example\n> A : teo@mail.example\n> Copie à : lou@mail.example, \"Max Roy\" , \"Ana Perez\" , \"Sam\nIto\"\n> Objet : Re: Sortie\n>\n> On part samedi ?\n";
const SIGNED_UNDER_QUOTE: &str =
    "Je transmets.\n\n> Bonjour,\n> La réunion est jeudi.\n\n>\nMax Roy\nDélégué de promotion\n";

#[rstest]
#[case::gmail_french_nbsp_colon(GMAIL_FR_NBSP, "Parfait, à demain.")]
#[case::gmail_french_wrapped_address(GMAIL_FR_WRAPPED, "Ça marche.")]
#[case::gmail_english_wrapped(GMAIL_EN_WRAPPED, "Sounds good.")]
#[case::outlook_english_header_block(OUTLOOK_EN, "Thanks a lot!")]
#[case::outlook_french_header_block(OUTLOOK_FR, "Merci pour le retour.")]
#[case::original_message_separator(ORIGINAL_MESSAGE, "Noted, I will call them.")]
#[case::zimbra_mail_original(ZIMBRA, "Je confirme ma présence.")]
#[case::samsung_glued_separator(SAMSUNG_GLUED, "Bien noté !")]
#[case::italian_attribution(ITALIAN, "Volentieri!")]
#[case::horde_quoting(HORDE, "Great, count me in.")]
#[case::attribution_without_address(BARE_ATTRIBUTION, "D'accord pour mardi.")]
#[case::trailing_angle_quote(TRAILING_QUOTE, "Yes.")]
#[case::bottom_posted_reply(BOTTOM_POSTED, "Oui, je les ai.")]
#[case::inline_reply_kept_whole(
    INLINE_REPLY,
    "> Can you bring the cables?\nYes, both of them.\n> And the projector?\nNo, it is broken."
)]
#[case::orange_quoted_header(ORANGE_QUOTED_HEADER, "Pas libre ce week-end.")]
#[case::signature_under_the_quote(SIGNED_UNDER_QUOTE, "Je transmets.")]
fn a_reply_shows_only_its_new_words(#[case] text: &str, #[case] want: &str) {
    assert_eq!(read(&mail("Re: x", text)).text, want);
}

#[rstest]
#[case::gmail(GMAIL_FR_NBSP)]
#[case::outlook(OUTLOOK_EN)]
#[case::trailing(TRAILING_QUOTE)]
fn a_reply_says_it_folded_a_quote(#[case] text: &str) {
    assert!(read(&mail("Re: x", text)).has_quoted);
}

#[rstest]
fn a_note_without_quote_folds_nothing() {
    assert!(!read(&mail("Hello", "Just a note.")).has_quoted);
}

// ---- Replies that add nothing of their own ----

const ONLY_QUOTE: &str = "\r\n\r\n> Le 21 déc. 2025 à 13:52, Ada Lune <ada@mail.example> a écrit :\r\n> \r\n> Le gâteau est au frigo.\r\n";

#[rstest]
fn a_reply_with_no_words_shows_what_it_quotes() {
    assert_eq!(
        read(&mail("Re: x", ONLY_QUOTE)).text,
        "Le gâteau est au frigo."
    );
}

#[rstest]
fn a_reply_with_no_words_says_so() {
    assert!(read(&mail("Re: x", ONLY_QUOTE)).quoted_only);
}

#[rstest]
fn a_reply_with_only_a_signature_and_a_quote_shows_the_quote() {
    assert_eq!(
        read(&mail(
            "Re: x",
            "Teo Blanc\nteo@mail.example\n+33 6 11 22 33 44\n\n> Tu viens ?\n"
        ))
        .text,
        "Tu viens ?"
    );
}

// ---- Forwards ----

const APPLE_FR_FORWARD: &str = "On y va mardi ?\r\n\r\nLéo Martin\r\nleo.martin@mail.example\r\n+33 6 11 22 33 44\r\n\r\n> Début du message réexpédié :\r\n> \r\n> De: Nina Roy <nina@club.example>\r\n> Objet: Apéro du club\r\n> Date: 8 octobre 2026 à 11:34:44 UTC+2\r\n> À: Léo Martin <leo.martin@mail.example>\r\n> \r\n> Bonjour à tous,\r\n> \r\n> Apéro mardi 20 octobre à 19h.\r\n";
const GMAIL_FORWARD: &str = "FYI\n\n---------- Forwarded message ---------\nFrom: Bank <alerts@bank.example>\nDate: Sun, Oct 11, 2026\nSubject: Statement\nTo: Teo <teo@mail.example>\n\nYour statement is ready.\n";
const GMAIL_FR_FORWARD_WRAPPED: &str = "Regarde ça !\n\n---------- Message transféré ----------\nDe : Ana Perez <ana@lab.example>\nDate : 23 juillet 2026 09:50\nObjet : RE : inscription au tournoi de ping-pong du club de quartier pour\nla saison prochaine\nÀ : Teo <teo@mail.example>\n\n\nBonjour,\n\nVotre équipe est inscrite.\n";
const ITALIAN_FORWARD: &str = "Guarda qui.\n\nInizio messaggio inoltrato:\n\n> Da: \"Lea Conti\" <lea@posta.example>\n> Data: 16 marzo 2026 20:54\n> A: Teo <teo@mail.example>\n> Oggetto: Battesimo\n>\n> Il battesimo è domenica.\n";
const THUNDERBIRD_FR_FORWARD: &str = "Dommage, je serai à la mer.\n\n-------- Message original --------\nSujet: \tAtelier poterie\nDate : \tThu, 2 Feb 2026 13:37:15 +0100\nDe : \tAna Perez <ana@lab.example>\nPour : \tpromo@lab.example\nCopie à : \tMax Roy <max@lab.example>,\nLou Petit <lou@lab.example>\n\n\n\nBonjour,\n\nIl reste deux places.\n";
const NESTED_FORWARD: &str = "---------- Forwarded message ---------\nDe : Eli Moreau <eli@mail.example>\nDate: mar. 15 févr. 2026 à 11:05\nSubject: Fwd: Message\nTo: Teo <teo@mail.example>, ana@mail.example <\nana@mail.example>\n\n\nReçu ce matin.\n\nDébut du message réexpédié :\n\n*De: *\"Roy, Nina\" <nina@garden.example>\n*Objet: *Message\n*Date: *15 février 2026 à 08:39\n*À: *Eli <eli@mail.example>\n\nLes tomates sont prêtes à cueillir.\n";
const LIST_QUOTED_FORWARD: &str = ">\n>To: tutor@school.example\n>Subject: Sortie botanique\n>\n>From: club@plants.example\n>Date: Thu, 11 Jul 2026 11:22:39 +0200\n>\n>Bonjour,\n>La sortie botanique a lieu dimanche.\n>\n\n--\nAna Perez\nSecrétariat du club\n";
const REPLY_TO_A_FORWARD: &str = "Tu as gardé les photos ?\n\nLe 17/09/2026 17:38, Teo Blanc a écrit :\n>\n> Début du message réexpédié :\n>\n>> De : sam@mail.example\n>> Objet : Vidéos\n>>\n>> Les vidéos du match.\n";

#[rstest]
#[case::apple_french(
    APPLE_FR_FORWARD,
    "On y va mardi ?\n\nBonjour à tous,\n\nApéro mardi 20 octobre à 19h."
)]
#[case::gmail(GMAIL_FORWARD, "FYI\n\nYour statement is ready.")]
#[case::gmail_french_wrapped_subject(
    GMAIL_FR_FORWARD_WRAPPED,
    "Regarde ça !\n\nBonjour,\n\nVotre équipe est inscrite."
)]
#[case::apple_italian(ITALIAN_FORWARD, "Guarda qui.\n\nIl battesimo è domenica.")]
#[case::thunderbird_french(
    THUNDERBIRD_FR_FORWARD,
    "Dommage, je serai à la mer.\n\nBonjour,\n\nIl reste deux places."
)]
#[case::forward_of_a_forward(
    NESTED_FORWARD,
    "Reçu ce matin.\n\nLes tomates sont prêtes à cueillir."
)]
#[case::quoted_under_a_list_tag(
    LIST_QUOTED_FORWARD,
    "Bonjour,\nLa sortie botanique a lieu dimanche."
)]
fn a_forward_reads_as_note_then_content(#[case] text: &str, #[case] want: &str) {
    assert_eq!(read(&mail("[promo] Fwd: x", text)).text, want);
}

#[rstest]
fn a_forward_is_flagged() {
    assert!(read(&mail("Fwd: x", GMAIL_FORWARD)).forwarded);
}

#[rstest]
fn a_forward_names_its_author() {
    assert_eq!(
        read(&mail("Fwd: x", GMAIL_FORWARD))
            .forwarded_from
            .as_deref(),
        Some("Bank")
    );
}

#[rstest]
fn a_forward_found_by_its_marker_needs_no_subject() {
    assert!(read(&mail("Apéro", APPLE_FR_FORWARD)).forwarded);
}

#[rstest]
fn a_reply_to_a_forward_shows_only_the_reply() {
    assert_eq!(
        read(&mail("Re: Fwd: Vidéos", REPLY_TO_A_FORWARD)).text,
        "Tu as gardé les photos ?"
    );
}

#[rstest]
#[case::marker("réexpédié")]
#[case::header("Objet:")]
#[case::quote_mark("> ")]
#[case::signature("leo.martin@mail.example")]
fn a_forward_shows_no_header(#[case] mark: &str) {
    assert!(!read(&mail("Fwd: x", APPLE_FR_FORWARD)).text.contains(mark));
}

// ---- Signatures ----

#[rstest]
#[case::rfc_delimiter("See you.\n-- \nMarc Petit\nCFO", "See you.")]
#[case::iphone("Ok!\n\nSent from my iPhone", "Ok!")]
#[case::french_iphone("D'accord\n\nEnvoyé de mon iPhone", "D'accord")]
#[case::orange_app("Oui\n\nEnvoyé depuis l'application Mail Orange", "Oui")]
#[case::outlook_mobile("Done\n\nGet Outlook for iOS", "Done")]
#[case::contact_block(
    "Voici le plan.\n\nTeo Blanc\nteo@mail.example\nlinkedin.com/in/teoblanc\n+336.11.22.33.44",
    "Voici le plan."
)]
#[case::title_line(
    "On garde le planning.\n\nMax Roy\nResponsable des ateliers de la Maison de quartier - antenne Nord\n+33 6 00 00 11 22",
    "On garde le planning."
)]
#[case::sign_off_name_kept(
    "Merci pour tout.\n\nBises,\nLou\nlou@mail.example\n06 11 22 33 44",
    "Merci pour tout.\n\nBises,\nLou"
)]
#[case::first_line_kept(
    "hop\nTeo Blanc\nteo@mail.example\nlinkedin.com/in/teoblanc\n+336.11.22.33.44",
    "hop"
)]
#[case::disclaimer(
    "Bien reçu.\n\nCe message et toutes les pièces jointes sont confidentiels.",
    "Bien reçu."
)]
#[case::environment_footer("Ci-joint.\n\nPensez à l'environnement avant d'imprimer.", "Ci-joint.")]
fn signatures_are_folded(#[case] text: &str, #[case] want: &str) {
    assert_eq!(read(&mail("x", text)).text, want);
}

#[rstest]
fn a_folded_signature_is_flagged() {
    assert!(read(&mail("x", "Ok!\n\nSent from my iPhone")).has_signature);
}

#[rstest]
fn a_message_that_is_only_a_signature_shows_it_without_its_delimiter() {
    assert_eq!(read(&mail("x", "-- \r\nTeo Blanc\r\n")).text, "Teo Blanc");
}

#[rstest]
#[case::short_list("Agenda:\nBudget\nHiring\nOffice move")]
#[case::number_in_a_sentence("Call me at +33 6 12 34 56 78 after five.")]
#[case::just_a_phone("06 11 22 33 44")]
fn content_is_not_a_signature(#[case] text: &str) {
    assert_eq!(read(&mail("x", text)).text, text);
}

// ---- Normalisation ----

#[rstest]
#[case::crlf("Line one\r\nLine two\r\n", "Line one\nLine two")]
#[case::lone_cr("Line one\rLine two", "Line one\nLine two")]
#[case::nbsp_and_thin_spaces("Prix\u{a0}:\u{202f}10\u{2009}€", "Prix : 10 €")]
#[case::zero_width("Bon\u{200b}jour\u{feff}", "Bonjour")]
#[case::invisible_only_lines(
    "Chers amis,\n\u{a0}\n\u{200b}\u{200c}\n\u{2007}\nLe marché ouvre.",
    "Chers amis,\n\nLe marché ouvre."
)]
#[case::entities(
    "l&#x27;homme du match &amp; l&#39;équipe",
    "l'homme du match & l'équipe"
)]
#[case::named_entities("D&eacute;but &agrave; 9h", "Début à 9h")]
#[case::hangul_filler("Mme Lou\u{3164}Petit", "Mme Lou Petit")]
#[case::leading_tabs("\tBonjour,\n\tVoici.", "Bonjour,\nVoici.")]
fn text_is_normalised(#[case] text: &str, #[case] want: &str) {
    assert_eq!(read(&mail("x", text)).text, want);
}

#[rstest]
fn quoted_printable_left_encoded_is_decoded() {
    let qp = "Voir la collection habill=C3=A9e =E2=86=92 ici.\nUne ligne tr=C3=A8s longue qui conti=\nnue ici, et une autre qui se poursu=\nit l=C3=A0 et qui va jusqu'au bo=\nut.";
    assert_eq!(
        read(&mail("x", qp)).text,
        "Voir la collection habillée → ici.\nUne ligne très longue qui continue ici, et une autre qui se poursuit là et qui va jusqu'au bout."
    );
}

#[rstest]
fn a_lone_equals_sign_is_not_quoted_printable() {
    assert_eq!(read(&mail("x", "a = b\nc = d")).text, "a = b\nc = d");
}

#[rstest]
fn output_never_holds_a_carriage_return() {
    assert!(!read(&mail("Re: x", OUTLOOK_EN)).text.contains('\r'));
}

// ---- Markdown, HTML and link artefacts ----

#[rstest]
#[case::image("![Logo](https://cdn.example/logo.png)\nBonjour", "Bonjour")]
#[case::link(
    "[Répondre](https://club.example/r/1) avant jeudi.",
    "Répondre avant jeudi."
)]
#[case::link_with_title(
    "[ Billets ](https://fest.example/tickets \"Tickets\") en vente",
    "Billets en vente"
)]
#[case::empty_anchor("[  ]( # )\nProgramme", "Programme")]
#[case::phone_link("Tél : [06 11 22 33 44](tel:06 11 22 33 44)", "Tél : 06 11 22 33 44")]
#[case::wrapped_target(
    "[Acheter](https://shop.example/products/pablo-cuir_ma\nrron) maintenant",
    "Acheter maintenant"
)]
#[case::heading(
    "## Match samedi\nRendez-vous à 10h",
    "Match samedi\nRendez-vous à 10h"
)]
#[case::bold("Le **match** est confirmé", "Le match est confirmé")]
#[case::hashtag_kept("#teamwork on Friday", "#teamwork on Friday")]
#[case::brackets_kept("Prix [indicatif] : 10 €", "Prix [indicatif] : 10 €")]
fn markdown_reads_as_words(#[case] text: &str, #[case] want: &str) {
    assert_eq!(read(&mail("x", text)).text, want);
}

#[rstest]
#[case::link_after_its_words(
    "Careers page <https://jobs.example/very/long/path?id=1>",
    "Careers page"
)]
#[case::mailto_annotation(
    "Écris à ana@lab.example <mailto:ana@lab.example>",
    "Écris à ana@lab.example"
)]
#[case::lone_link_kept("<https://docs.example/sheet>", "https://docs.example/sheet")]
#[case::pasted_image(
    "Voir ci-dessous :\n<GraphiqueColle-1.png>\nMerci",
    "Voir ci-dessous :\nMerci"
)]
#[case::outlook_cid("Plan joint [cid:image001.png@01D2] ici", "Plan joint ici")]
#[case::gmail_image_name("[image: Logo Club]\nBienvenue", "Bienvenue")]
#[case::signature_tracker(
    "Merci.\nhttps://cloud.letsignit.com/collect/bc/abc123?p=xyz",
    "Merci."
)]
#[case::stray_break_tag("Merci de ne pas répondre.<BR>", "Merci de ne pas répondre.")]
#[case::comparison_kept("a < b and c > d", "a < b and c > d")]
fn link_and_tag_artefacts_are_removed(#[case] text: &str, #[case] want: &str) {
    assert_eq!(read(&mail("x", text)).text, want);
}

#[rstest]
fn a_personal_link_stays() {
    assert_eq!(
        read(&mail(
            "x",
            "Le doc : https://docs.example/spreadsheets/d/1abc/edit"
        ))
        .text,
        "Le doc : https://docs.example/spreadsheets/d/1abc/edit"
    );
}

// ---- Source choice: raw HTML, HTML-only, flattened plain parts ----

const RAW_PLAIN: &str = "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\" \"https://mail.events.example/ls/click?upn=abc123\">\r\n<html dir=\"ltr\" lang=\"en\"><head><title>Forum 2026</title><link rel=\"preload\" as=\"image\" href=\"https://mail.events.example/ls/click?upn=def456\"></head>\r\n<body><div><p>Dear guest,</p><p>Your registration is confirmed.</p></div></body></html>";

#[rstest]
fn raw_html_in_the_plain_part_reads_from_the_html_part() {
    let m = Mail {
        text: RAW_PLAIN,
        html: Some("<p>Registration confirmed for the Forum.</p>"),
        ..Default::default()
    };
    assert_eq!(read(&m).text, "Registration confirmed for the Forum.");
}

#[rstest]
fn raw_html_without_an_html_part_reads_as_html() {
    assert_eq!(
        read(&mail("x", RAW_PLAIN)).text,
        "Dear guest,\n\nYour registration is confirmed."
    );
}

#[rstest]
fn an_empty_plain_part_reads_from_the_html_cut_at_its_quote() {
    let m = Mail {
        text: "\r\n",
        html: Some(
            r#"<div dir="ltr">Merci !</div><div class="gmail_quote"><div>Le lun., Ana a écrit :</div><blockquote>Ci-joint.</blockquote></div>"#,
        ),
        ..Default::default()
    };
    assert_eq!(read(&m).text, "Merci !");
}

#[rstest]
fn a_flattened_plain_part_reads_from_the_html_that_marks_the_quote() {
    let m = Mail {
        text: "Parfait\nLe 2 nov. 2026 à 08:27, Ana <ana@mail.example> a écrit :Salut ! Tu as la clé ?",
        html: Some(
            r#"<div>Parfait</div><br><blockquote type="cite"><div>Le 2 nov. 2026 à 08:27, Ana a écrit :</div><div>Salut ! Tu as la clé ?</div></blockquote>"#,
        ),
        ..Default::default()
    };
    assert_eq!(read(&m).text, "Parfait");
}

#[rstest]
fn a_link_dump_reads_from_its_html() {
    let m = Mail {
        text: "https://news.example/view?id=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\nhttps://news.example/img?id=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        html: Some("<h1>Le marché rouvre samedi</h1><p>Venez nombreux.</p>"),
        ..Default::default()
    };
    assert_eq!(read(&m).text, "Le marché rouvre samedi\n\nVenez nombreux.");
}

#[rstest]
fn a_personal_reply_opening_on_a_link_keeps_its_quote_structure() {
    assert_eq!(
        read(&mail(
            "Re: vidéo",
            "https://videos.example/watch?v=abcdefghijklmnopqrstuvwxyz0123456789\r\n\r\nLe 14/04/2026 20:25, Teo a écrit :\r\n> Tu as vu ?\r\n"
        ))
        .text,
        "https://videos.example/watch?v=abcdefghijklmnopqrstuvwxyz0123456789"
    );
}

// ---- Bulk mail ----

const MAILJET_TOWN: &str = "<<Camille Durand - Maire de Villeneuve>> [https://x1abc.mjt.lu/img/x1abc/b/portrait.png]\n\u{a0}\n\u{200b}\u{200c}\nChers Habitants,\n\u{a0}\nLe nouveau marché ouvre samedi.\n<<logo>> [https://x1abc.mjt.lu/img/x1abc/b/logo.png]\n";
const NEWSLETTER: &str = "Voir la version en ligne\n\nLa lettre du quartier\n\nLa mairie consulte les habitants. En savoir plus (https://quartier.example/consultation-sur-les-pistes-cyclables-du-quartier)\n\n>> Je participe\n\nVous recevez cet email car vous êtes inscrit.\nSe désabonner (https://quartier.example/unsubscribe?u=1)";

#[rstest]
fn a_mailjet_placeholder_reads_as_its_headline() {
    assert_eq!(
        read(&mail("x", MAILJET_TOWN)).text,
        "Camille Durand - Maire de Villeneuve\n\nChers Habitants,\n\nLe nouveau marché ouvre samedi."
    );
}

#[rstest]
#[case::view_online("version en ligne")]
#[case::button_target("consultation-sur")]
#[case::arrows(">>")]
#[case::footer("désabonner")]
fn newsletter_boilerplate_is_removed(#[case] mark: &str) {
    assert!(!read(&mail("x", NEWSLETTER)).text.contains(mark));
}

#[rstest]
fn a_newsletter_keeps_its_words() {
    assert_eq!(
        read(&mail("x", NEWSLETTER)).text,
        "La lettre du quartier\n\nLa mairie consulte les habitants. En savoir plus\n\nJe participe"
    );
}

#[rstest]
fn a_newsletter_is_flagged_bulk() {
    assert!(read(&mail("x", NEWSLETTER)).bulk);
}

#[rstest]
fn a_list_post_is_bulk() {
    let m = Mail {
        text: "Hello list.",
        listed: true,
        ..Default::default()
    };
    assert!(read(&m).bulk);
}

#[rstest]
fn a_personal_note_is_not_bulk() {
    assert!(!read(&mail("x", "See you at nine.")).bulk);
}

// ---- Never blank ----

#[rstest]
#[case::plain("ok merci", "ok merci")]
#[case::emoji("👍", "👍")]
#[case::html_only_text("", "Bien reçu")]
fn short_messages_are_never_blank(#[case] text: &str, #[case] want: &str) {
    let m = Mail {
        text,
        html: Some("<p>Bien reçu</p>"),
        ..Default::default()
    };
    assert_eq!(read(&m).text, want);
}

#[rstest]
#[case::empty("")]
#[case::only_spaces(" \r\n\u{a0}\r\n")]
#[case::only_quote_bars("\r\n>  \r\n> \r\n")]
fn a_message_without_words_reads_empty(#[case] text: &str) {
    assert_eq!(read(&mail("x", text)).text, "");
}

#[rstest]
fn an_html_part_of_only_styles_reads_empty() {
    let m = Mail {
        text: "\r\n",
        html: Some(
            "<html><head><style>p{margin:0}</style></head><body><p>&nbsp;</p></body></html>",
        ),
        ..Default::default()
    };
    assert_eq!(read(&m).text, "");
}

#[rstest]
fn stray_quote_bars_in_a_note_are_dropped() {
    assert_eq!(
        read(&mail("x", ">\nBonjour,\nVoici l'offre.")).text,
        "Bonjour,\nVoici l'offre."
    );
}

// ---- Stored flags ----

#[rstest]
fn flags_survive_storage() {
    let r = read(&mail("Fwd: x", GMAIL_FORWARD));
    assert_eq!(
        Reading::from_parts(r.text.clone(), r.flags(), r.forwarded_from.clone()),
        r
    );
}

// ---- Determinism: near-identical letters read alike ----

#[rstest]
fn the_same_letter_sent_twice_reads_the_same() {
    let a = read(&mail("Re: x", GMAIL_FR_NBSP)).text;
    let b = read(&mail("Re: x", &GMAIL_FR_NBSP.replace("\r\n", "\n"))).text;
    assert_eq!(a, b);
}

// ---- Mobile and Outlook "original message" separators ----

#[rstest]
#[case::outlook_mobile_french(
    "Ok pour jeudi.\n\n------ Message d'origine------\nDe: Max <max@mail.example>\nEnvoyé: lundi 5 octobre 2026 09:00\nÀ: Teo\nObjet: Réunion\n\nOn se voit jeudi ?\n"
)]
#[case::separator_last_line("Ok pour jeudi.\n------ Message d'origine------")]
#[case::glued_to_the_reply(
    "Ok pour jeudi.------ Message d'origine------De: Max <max@mail.example> Envoyé: lundi À: Teo Objet: Réunion On se voit jeudi ?"
)]
#[case::spaced(
    "Ok pour jeudi.\n\n-----Message d'origine-----\nDe : Max\nEnvoyé : lundi\nÀ : Teo\nObjet : Réunion\n\nJeudi ?\n"
)]
#[case::non_breaking_spaces(
    "Ok pour jeudi.\n\n------\u{a0}Message d'origine\u{a0}------\nDe\u{a0}: Max\n\nJeudi ?\n"
)]
#[case::message_original(
    "Ok pour jeudi.\n\n-------- Message original --------\nSujet: Réunion\nDate : lundi\nDe : Max\n\nJeudi ?\n"
)]
#[case::english(
    "Ok pour jeudi.\n\n-----Original Message-----\nFrom: Max\nSent: Monday\nTo: Teo\nSubject: Meeting\n\nThursday?\n"
)]
#[case::blackberry_footer_above(
    "Ok pour jeudi.\nSent from my BlackBerry® smartphone\n-----Original Message-----\nFrom: Max <max@mail.example>\nDate: Mon\nTo: Teo\nSubject: Meeting\n\nThursday?\n"
)]
fn a_short_reply_ends_before_its_original_message(#[case] text: &str) {
    assert_eq!(read(&mail("Re: Réunion", text)).text, "Ok pour jeudi.");
}

#[rstest]
fn a_bounce_keeps_its_report_without_the_returned_mail() {
    let bounce = "<<< 550 5.1.1 <ana@school.example>... User unknown\n\n   ----- Original message follows -----\n\nReturn-Path: <teo@mail.example>\nReceived: from relay.example\nSubject: Photos\n\nVoici les photos.\n";
    assert_eq!(
        read(&mail("Returned mail", bounce)).text,
        "<<< 550 5.1.1 <ana@school.example>... User unknown"
    );
}

// ---- Style sheets never reach a bubble ----

const FONT_FACE: &str = "@font-face { font-family: 'Sans'; font-style: normal; font-weight: 100; src: url('https://static.shop.example/fonts/Sans-100.eot'); src: url('https://static.shop.example/fonts/Sans-100.eot?#iefix') format('embedded-opentype'); }\n@font-face { font-family: 'Sans'; font-weight: 400; src: url('https://static.shop.example/fonts/Sans-400.woff'); }\n";

#[rstest]
fn a_plain_part_opening_on_css_reads_from_the_html() {
    let text = format!("{FONT_FACE}Votre commande\nN° de commande : AB12CD");
    let m = Mail {
        text: &text,
        html: Some("<p>Votre commande AB12CD a été acceptée.</p>"),
        ..Default::default()
    };
    assert_eq!(read(&m).text, "Votre commande AB12CD a été acceptée.");
}

#[rstest]
#[case::font_face_without_html(&format!("{FONT_FACE}Votre commande a été acceptée."), "Votre commande a été acceptée.")]
#[case::preheader_then_media(
    "Hommage au fondateur @media only screen and (min-width: 620px) { .u-row { width: 600px !important; } }\nLe programme de juin.",
    "Hommage au fondateur\nLe programme de juin."
)]
#[case::unterminated_media(
    "Merci pour votre achat.\n\n@media print{\n#_t {\nbackground-image:\nurl('https://track.example/p')\n}\n",
    "Merci pour votre achat."
)]
#[case::last_declaration_on_the_brace(
    "Bien reçu.\n\n@media print{\n#_t {\nbackground-image:\nurl('https://track.example/p')\n}\n}\n\ndiv.OutlookMessageHeader {\nbackground-image: url('https://track.example/f')\n}\n",
    "Bien reçu."
)]
#[case::selector_list(
    "a:link, span.MsoHyperlink\n{mso-style-priority:99;\ncolor:blue;}\nBonjour,\nVoici le devis.",
    "Bonjour,\nVoici le devis."
)]
#[case::outlook_vml(
    "Je suis d'accord.\nv\\:* {behavior:url(#default#VML);}\no\\:* {behavior:url(#default#VML);}",
    "Je suis d'accord."
)]
#[case::import_without_semicolon(
    "Paiement reçu    @import url(https://fonts.example/css2?family=Sans&display=swap)    @media only screen and (max-width: 620px) { h1 { font-size: 28px !important; } }",
    "Paiement reçu"
)]
#[case::inline_style_attribute(
    "Merci d'avoir choisi le service\" style=\"color: #fff; font-weight: 600 !important;\"",
    "Merci d'avoir choisi le service"
)]
#[case::conditional_comment_marks(
    "<!--[if mso]>\nBonjour,\n<![endif]-->\nLe colis est parti.",
    "Bonjour,\nLe colis est parti."
)]
#[case::braces_in_prose_kept("Use {x, y} as the pair.", "Use {x, y} as the pair.")]
#[case::arrow_kept("Paris --> Lyon, puis retour.", "Paris --> Lyon, puis retour.")]
#[case::code_in_prose_kept(
    "Ajoute if (x) { return; } dans la boucle.",
    "Ajoute if (x) { return; } dans la boucle."
)]
fn css_is_removed_from_plain_text(#[case] text: &str, #[case] want: &str) {
    assert_eq!(read(&mail("x", text)).text, want);
}

#[rstest]
#[case::style_in_body(r#"<html><body><style>p { color: red; }</style><p>Votre colis arrive demain.</p></body></html>"#)]
#[case::uppercase_style_with_attributes(r#"<HTML><BODY><STYLE type="text/css" media="screen">P { COLOR: red; }</STYLE><P>Votre colis arrive demain.</P></BODY></HTML>"#)]
#[case::outlook_conditional(r#"<html><body><!--[if mso]><style>td { font-family: Arial; }</style><xml><o:OfficeDocumentSettings><o:AllowPNG/></o:OfficeDocumentSettings></xml><![endif]--><p>Votre colis arrive demain.</p></body></html>"#)]
#[case::malformed_head(r#"<html><head><title>Commande</title><style>@font-face { font-family: 'Sans'; }</style><body><p>Votre colis arrive demain.</p></body></html>"#)]
#[case::script(r#"<html><body><script>var t = { a: 1 };</script><p>Votre colis arrive demain.</p></body></html>"#)]
fn html_style_sheets_never_read_as_text(#[case] html: &str) {
    let m = Mail {
        text: "",
        html: Some(html),
        ..Default::default()
    };
    assert_eq!(read(&m).text, "Votre colis arrive demain.");
}

// ---- HTML layout breaks: fragments join, real breaks stay ----

fn from_html(html: &str) -> String {
    read(&Mail {
        text: "",
        html: Some(html),
        ..Default::default()
    })
    .text
}

#[rstest]
#[case::source_newline_inside_a_cell(
    "<table><tr><td>Votre\n   <span class=\"x\">colis</span>&nbsp;\n</td></tr></table>",
    "Votre colis"
)]
#[case::source_newlines_in_a_heading(
    "<h2>Des nouvelles de\n votre\n <span>colis ?</span></h2>",
    "Des nouvelles de votre colis ?"
)]
#[case::label_broken_by_br(
    "<p>15 jours pour<br>changer d'avis</p>",
    "15 jours pour changer d'avis"
)]
#[case::lone_question_mark("<p>Votre colis<br>?</p>", "Votre colis ?")]
#[case::lone_comma("<p>Contactez Atelier Nord<br>,</p>", "Contactez Atelier Nord,")]
#[case::address_kept(
    "<p>Livraison à :<br>6 rue des Lilas<br>75011 Paris</p>",
    "Livraison à :\n6 rue des Lilas\n75011 Paris"
)]
#[case::sign_off_kept("<p>Bien à vous,<br>Nora Lind</p>", "Bien à vous,\nNora Lind")]
#[case::greeting_line_kept(
    "<p>Bonjour,<br>votre colis est arrivé.</p>",
    "Bonjour,\nvotre colis est arrivé."
)]
#[case::list_kept("<ul><li>pommes</li><li>poires</li></ul>", "• pommes\n• poires")]
#[case::table_rows_kept(
    "<table><tr><td>Total</td><td>10 €</td></tr><tr><td>Port</td><td>offert</td></tr></table>",
    "Total 10 €\nPort offert"
)]
#[case::preformatted_lines_kept("<pre>Ligne une\nLigne deux</pre>", "Ligne une\nLigne deux")]
#[case::long_wrapped_sentences_kept(
    "<p>Votre commande a bien été expédiée par le vendeur hier soir<br>et arrivera chez vous sous trois jours ouvrés.</p>",
    "Votre commande a bien été expédiée par le vendeur hier soir\net arrivera chez vous sous trois jours ouvrés."
)]
fn html_layout_breaks(#[case] html: &str, #[case] want: &str) {
    assert_eq!(from_html(html), want);
}

#[rstest]
fn a_plain_part_keeps_its_own_line_breaks() {
    assert_eq!(read(&mail("x", "Prix\nunitaire")).text, "Prix\nunitaire");
}

// ---- Unfilled mailing merge tags ----

#[rstest]
#[case::mailchimp(
    "Hello *|FIRST NAME|*,\nSee you at the forum.",
    "Hello,\nSee you at the forum."
)]
#[case::mailchimp_fname(
    "Bonjour *|FNAME|* !\nLe forum ouvre lundi.",
    "Bonjour !\nLe forum ouvre lundi."
)]
#[case::handlebars(
    "Hi {{ first_name }},\nYour order shipped.",
    "Hi,\nYour order shipped."
)]
#[case::percent(
    "Dear %%FirstName%%,\nThanks for joining.",
    "Dear,\nThanks for joining."
)]
#[case::bracket_field("Hello [FNAME],\nWelcome aboard.", "Hello,\nWelcome aboard.")]
#[case::brace_field("Bonjour {PRENOM},\nBienvenue.", "Bonjour,\nBienvenue.")]
#[case::ordinary_brackets_kept("Prix [indicatif] : 10 €", "Prix [indicatif] : 10 €")]
#[case::ordinary_braces_kept("Use {x, y} as the pair.", "Use {x, y} as the pair.")]
#[case::filled_greeting_kept("Hello Nora,\nSee you.", "Hello Nora,\nSee you.")]
fn unfilled_merge_tags_are_dropped(#[case] text: &str, #[case] want: &str) {
    assert_eq!(read(&mail("x", text)).text, want);
}

// ---- Classes found by reading real mail one by one ----

#[rstest]
#[case::gateway_banner(
    "Vous ne recevez pas souvent de courriers de la part de x@cab.example. Découvrez pourquoi cela est important\nBonjour,\nVoici l'ordonnance.",
    "Bonjour,\nVoici l'ordonnance."
)]
#[case::thunderbird_emphasis(
    "Rendez-vous *jeudi* au *café du\nPort*.",
    "Rendez-vous jeudi au café du\nPort."
)]
#[case::italic_emphasis(
    "le livre */Les Jardins du Nord/*, paru hier",
    "le livre Les Jardins du Nord, paru hier"
)]
#[case::multiplication_kept("2 * 3 * 4 = 24", "2 * 3 * 4 = 24")]
#[case::outlook_download_footer(
    "7 - Teo\nTéléchargez Outlook pour iOS<https://aka.ms/abc>",
    "7 - Teo"
)]
#[case::blackberry_footer(
    "On se retrouve à 19h.\nCe message a été envoyé depuis un terminal BlackBerry",
    "On se retrouve à 19h."
)]
#[case::antivirus_footer(
    "Voici la photo\nNina\n\nL'absence de virus dans ce courrier électronique a été vérifiée par le logiciel antivirus.",
    "Voici la photo\nNina"
)]
fn junk_found_in_real_mail_is_removed(#[case] text: &str, #[case] want: &str) {
    assert_eq!(read(&mail("x", text)).text, want);
}

#[rstest]
#[case::old_gmail_dated(
    "Super idée.\n\n2011/9/20 Max Roy <max@mail.example>\n> On fait quoi samedi ?\n",
    "Super idée."
)]
#[case::old_gmail_dated_wrapped(
    "Bonne idée.\nTeo\n2011/9/13 Ana Perez <ana@mail.example\n>\n> Un indice ?\n",
    "Bonne idée.\nTeo"
)]
#[case::android_reply_separator(
    "Oui, très joli !\n----- Reply message -----\nDe : \"Max\" <max@mail.example>\nDate : sam. 24 déc.\nObjet : Photo\n\nLa photo.\n",
    "Oui, très joli !"
)]
#[case::glued_attribution(
    "Parfait\nLe 2 nov. 2026 à 08:27, Ana <ana@mail.example> a écrit :Salut ! Tu as la clé ?",
    "Parfait"
)]
fn more_attribution_formats_are_folded(#[case] text: &str, #[case] want: &str) {
    assert_eq!(read(&mail("Re: x", text)).text, want);
}

#[rstest]
#[case::lowercase_sentence_is_not_a_title(
    "voici le plan relu par Max\nTeo Blanc\nteo@mail.example\n+336.11.22.33.44",
    "voici le plan relu par Max"
)]
#[case::glued_dash_line_kept(
    "Le chat dort sur le canapé--\nLou\n06 00 00 33 44",
    "Le chat dort sur le canapé--"
)]
#[case::one_line_flattened_signature(
    "Bonne remarque.\nTeo Blancteo@mail.examplelinkedin.com/in/teoblanc+336.11.22.33.44",
    "Bonne remarque."
)]
#[case::postal_address(
    "Pense à remercier le club dans le compte rendu.\nA+\nAna Perez\nAtelier des Plantes\nMailing address:\n99999 Villeneuve Cedex 1, France",
    "Pense à remercier le club dans le compte rendu.\nA+\nAna Perez"
)]
#[case::french_closing_and_picture(
    "FYI\nSincèrement,\nNina Roy\nGestionnaire\n[cid:image002.jpg@01D9F124.897D30A0]",
    "FYI\nSincèrement,\nNina Roy\nGestionnaire"
)]
fn signature_edge_cases(#[case] text: &str, #[case] want: &str) {
    assert_eq!(read(&mail("x", text)).text, want);
}

const FORWARD_WITH_ADDRESS_LIST: &str = "---------- Message transféré ----------\nDe : Nina Roy <nina@mail.example>\nDate : 29 janvier 2026 21:34\nObjet : Photos\nÀ : \"Roy (M. et Mme)\" <roy@mail.example>, Ana Perez\n<ana@mail.example>, \"Max Roy\" <\nmax@mail.example>, Lou <lou@mail.example>\n\nVoici une deuxième photo\nNina\n";
const FORWARD_CHAIN: &str = "LOL :\nDébut du message réexpédié :\n> De : Ana Perez <ana@school.example>\n> Date : 2 septembre 2026 11:00\n> À : Max Roy <max@school.example>\n> Objet : Réexp : Potager\n> \n> je pense que ça va te plaire\n> \n> -------- Message original --------\n> Sujet:\tPotager\n> Date :\tThu, 01 Sep 2026 23:40:41 +0100\n> De :\tLou Petit <lou@school.example>\n> Pour :\tpromo@school.example\n> \n> Chère équipe, le potager partagé ouvre lundi.\n";

#[rstest]
#[case::address_list_wrapped_over_lines(
    FORWARD_WITH_ADDRESS_LIST,
    "Voici une deuxième photo\nNina"
)]
#[case::innermost_content_of_a_chain(
    FORWARD_CHAIN,
    "LOL :\n\nje pense que ça va te plaire\n\nChère équipe, le potager partagé ouvre lundi."
)]
fn forward_edge_cases(#[case] text: &str, #[case] want: &str) {
    assert_eq!(read(&mail("Fwd: x", text)).text, want);
}

#[rstest]
fn a_plain_stub_asking_for_html_reads_from_the_html() {
    let m = Mail {
        text: "Sorry, this email cant be viewed without HTML",
        html: Some("<p>The garden opens at nine.</p>"),
        ..Default::default()
    };
    assert_eq!(read(&m).text, "The garden opens at nine.");
}

#[rstest]
fn an_inline_reply_in_html_keeps_its_answers() {
    let m = Mail {
        text: "ok pour moi.\nSur ton point :- Les horaires.On ouvre à neuf heures.",
        html: Some(
            r#"<div>ok pour moi.</div><div>Sur ton point :</div><blockquote type="cite"><div>- Les horaires.</div></blockquote><div>On ouvre à neuf heures.</div>"#,
        ),
        ..Default::default()
    };
    assert!(read(&m).text.contains("On ouvre à neuf heures."));
}

#[rstest]
#[case::footer_you_received(
    "User Ana changed issue:\nAB-1 Button misaligned\nvotes: +1\nYou received this message because you subscribe to notification events.",
    "User Ana changed issue:\nAB-1 Button misaligned\nvotes: +1"
)]
#[case::view_online_wrapped_word(
    "Si vous ne visualisez pas correctement l'e-mail, cliquez\nici\nChers amis,\nLa réunion est jeudi.\nSe désabonner",
    "Chers amis,\nLa réunion est jeudi."
)]
#[case::short_online_banner(
    "La lettre du jour\nVoir la lettre en ligne - Archives\nDimanche 26 décembre\nSe désabonner",
    "La lettre du jour\nDimanche 26 décembre"
)]
#[case::long_link_after_words(
    "Votre recherche enregistrée\nVoir cette annonce https://ads.example/view/1111111111?refId=aaaaaaaa-bbbb&trk=eml-xyz\nSe désabonner",
    "Votre recherche enregistrée\nVoir cette annonce"
)]
fn more_bulk_boilerplate_is_removed(#[case] text: &str, #[case] want: &str) {
    assert_eq!(read(&mail("x", text)).text, want);
}

#[rstest]
fn a_web_page_menu_is_not_text() {
    let page = "<!DOCTYPE html><html><head><title>Tennis</title></head><body><ul><li>home</li><li>prices</li><li>rules</li><li>profile</li><li>account</li><li></li></ul><p>Logout</p><h2>Step 2 of 3</h2><p>Your booking is being processed.</p></body></html>";
    assert_eq!(
        read(&mail("x", page)).text,
        "Step 2 of 3\n\nYour booking is being processed."
    );
}
