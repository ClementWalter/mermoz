//! Sender verification from the `Authentication-Results` header Gmail adds on receipt.
//!
//! Only Gmail's own header (authserv-id `mx.google.com`) is trusted: any other copy
//! can be written by the sender. A message is "pass" when DMARC passes, or when DKIM
//! or SPF pass for the From domain. It is "fail" only when the failure is explicit:
//! DMARC fails for a domain whose policy asks receivers to quarantine or reject such
//! mail, or, without DMARC, hard SPF and DKIM both fail for the From domain. Anything
//! else is "none": no header, or a failure the domain itself says can be legitimate
//! (policy `p=none`), which is what forwarding produces (a copy relayed by iCloud or
//! a mailing list breaks the original signature). A pass from a
//! subdomain of a platform where anyone can open an account (a Jira site, a shop, a
//! hosted page) is "shared": the domain is real, but it vouches for nobody. A
//! domain that glues a word of the sender's name to a filler word ("GitHub" from
//! `noreply-github.co`) is "lookalike": it proves only that a copycat domain sent it.

/// Platforms that give every customer their own subdomain to send from.
const SHARED_PLATFORMS: &[&str] = &[
    "atlassian.net",
    "zendesk.com",
    "freshdesk.com",
    "myshopify.com",
    "github.io",
    "gitlab.io",
    "vercel.app",
    "netlify.app",
    "pages.dev",
    "web.app",
    "firebaseapp.com",
    "herokuapp.com",
    "sharepoint.com",
    "onmicrosoft.com",
    "wixsite.com",
    "squarespace.com",
    "notion.site",
    "typeform.com",
    "hubspotemail.net",
    "mailchimpapp.com",
    "substack.com",
    "glitch.me",
];

/// Whether the address sends from a customer subdomain of a shared platform.
pub fn is_shared_platform(email: &str) -> bool {
    let d = domain_of(email);
    SHARED_PLATFORMS
        .iter()
        .any(|p| d.ends_with(&format!(".{p}")))
}

/// Words copycat domains add around a brand to look official.
const FILLER_WORDS: &[&str] = &[
    "noreply",
    "no",
    "reply",
    "notif",
    "notification",
    "notifications",
    "notify",
    "app",
    "apps",
    "mail",
    "email",
    "secure",
    "security",
    "support",
    "help",
    "helpdesk",
    "login",
    "signin",
    "account",
    "accounts",
    "verify",
    "verification",
    "auth",
    "id",
    "alert",
    "alerts",
    "service",
    "services",
    "team",
    "billing",
    "official",
    "online",
    "update",
    "center",
    "portal",
    "admin",
];

/// Second-level labels under which a country code registers names (`example.co.uk`).
const COUNTRY_SECOND_LEVELS: &[&str] = &["co", "com", "org", "net", "gov", "ac", "edu"];

/// The domain an organisation registered: `email.github.com` → `github.com`,
/// `notif.x.co.uk` → `x.co.uk`.
pub fn registered_domain(domain: &str) -> String {
    let labels: Vec<&str> = domain.split('.').collect();
    let keep = match labels.as_slice() {
        [.., _, sld, cc] if cc.len() == 2 && COUNTRY_SECOND_LEVELS.contains(sld) => 3,
        _ => 2,
    };
    labels[labels.len().saturating_sub(keep)..].join(".")
}

/// The name a domain was registered under, without its suffix: `notif-x.co.uk` → `notif-x`.
fn registered_label(domain: &str) -> String {
    registered_domain(domain)
        .split('.')
        .next()
        .unwrap_or("")
        .to_string()
}

/// The domain a sender name usually writes from, when `email` is not on it. `counts`
/// holds how many received messages carry that name, per sending domain: a domain
/// the name rarely uses next to one it uses all the time is a copycat.
pub fn usual_domain(counts: &[(String, i64)], email: &str) -> Option<String> {
    let mut by_org: std::collections::BTreeMap<String, i64> = Default::default();
    for (d, n) in counts {
        *by_org.entry(registered_domain(d)).or_default() += n;
    }
    let mine = registered_domain(&domain_of(email));
    let own = by_org.get(&mine).copied().unwrap_or(0);
    let (usual, n) = by_org.into_iter().max_by_key(|(_, n)| *n)?;
    (usual != mine && n >= 3 && own * 3 <= n).then_some(usual)
}

/// Whether the domain pairs a word of the display name with filler, like
/// "GitHub" <noreply@noreply-github.co> or "Slack" <noreply@slack-app.cloud>.
pub fn is_lookalike(name: &str, email: &str) -> bool {
    if name.contains('@') {
        return false;
    }
    let domain = domain_of(email);
    let label = registered_label(&domain);
    let parts: Vec<&str> = label.split(['-', '_']).filter(|p| !p.is_empty()).collect();
    let name = name.to_lowercase();
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).collect();
    let names_brand = parts.iter().any(|p| p.len() >= 4 && words.contains(p));
    let has_filler = parts
        .iter()
        .any(|p| FILLER_WORDS.contains(p) && !words.contains(p));
    parts.len() >= 2 && names_brand && has_filler
}

/// A stored verdict as shown: a pass from a shared platform reads as "shared", and a
/// pass or an inconclusive check from a copycat domain reads as "lookalike".
pub fn shown(verdict: Option<&str>, name: &str, email: &str) -> Option<String> {
    verdict.map(|v| match v {
        "pass" | "none" if is_lookalike(name, email) => "lookalike".into(),
        "pass" if is_shared_platform(email) => "shared".into(),
        _ => v.into(),
    })
}

/// The `Authentication-Results` header values of a raw message, unfolded, in order.
pub fn results_from_raw(raw: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(raw);
    let head = text
        .split("\r\n\r\n")
        .next()
        .unwrap_or("")
        .split("\n\n")
        .next()
        .unwrap_or("");
    let mut out: Vec<String> = vec![];
    let mut current: Option<String> = None;
    for line in head.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(c) = current.as_mut() {
                c.push(' ');
                c.push_str(line.trim());
            }
            continue;
        }
        if let Some(c) = current.take() {
            out.push(c);
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("authentication-results")
        {
            current = Some(value.trim().to_string());
        }
    }
    if let Some(c) = current {
        out.push(c);
    }
    out
}

fn domain_of(email: &str) -> String {
    email
        .rsplit('@')
        .next()
        .unwrap_or("")
        .trim()
        .trim_end_matches('>')
        .to_lowercase()
}

/// Relaxed alignment: the same domain, or one a subdomain of the other.
fn aligned(a: &str, b: &str) -> bool {
    !a.is_empty()
        && !b.is_empty()
        && (a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}")))
}

/// "pass", "fail" or "none" for a message from `from_email`, given its
/// `Authentication-Results` values (as received, topmost first).
pub fn verdict(results: &[String], from_email: &str) -> &'static str {
    let Some(r) = results.iter().find(|v| {
        v.split(';')
            .next()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("mx.google.com")
    }) else {
        return "none";
    };
    let from = domain_of(from_email);
    let (mut dmarc, mut enforced, mut dkim_pass, mut dkim_fail, mut spf_pass, mut spf_fail) =
        (None, false, false, false, false, false);
    for clause in r.split(';').skip(1) {
        let words: Vec<&str> = clause.split_whitespace().collect();
        let Some((method, result)) = words.first().and_then(|w| w.split_once('=')) else {
            continue;
        };
        let prop = |key: &str| {
            words
                .iter()
                // Policy properties sit in a comment: "(p=REJECT sp=NONE dis=NONE)".
                .find_map(|w| w.trim_start_matches('(').strip_prefix(key))
                .map(|v| {
                    v.trim_start_matches('@')
                        .trim_matches(|c| c == '"' || c == ')' || c == '(')
                        .to_lowercase()
                })
                .unwrap_or_default()
        };
        let result = result.to_lowercase();
        match method.to_lowercase().as_str() {
            "dmarc" => {
                let strict = |v: String| matches!(v.as_str(), "quarantine" | "reject");
                enforced = strict(prop("p=")) || strict(prop("dis="));
                dmarc = Some(result)
            }
            "dkim" => {
                let d = {
                    let i = prop("header.i=");
                    if i.is_empty() {
                        prop("header.d=")
                    } else {
                        domain_of(&i)
                    }
                };
                if result == "pass" && aligned(&d, &from) {
                    dkim_pass = true;
                } else if result == "fail" && aligned(&d, &from) {
                    dkim_fail = true;
                }
            }
            "spf" => {
                let d = domain_of(&prop("smtp.mailfrom="));
                if result == "pass" && aligned(&d, &from) {
                    spf_pass = true;
                } else if result == "fail" {
                    spf_fail = true;
                }
            }
            _ => {}
        }
    }
    match dmarc.as_deref() {
        Some("pass") => "pass",
        Some("fail") if enforced => "fail",
        Some("fail") => "none",
        _ if dkim_pass || spf_pass => "pass",
        _ if dkim_fail && spf_fail => "fail",
        _ => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    const AWS: &str = "mx.google.com; dkim=pass header.i=@amazonaws.com header.s=x; spf=pass (google.com: domain of bounce@amazonaws.com designates 1.2.3.4) smtp.mailfrom=bounce@amazonaws.com; dmarc=pass (p=QUARANTINE sp=QUARANTINE dis=NONE) header.from=amazonaws.com";
    const SPOOF: &str = "mx.google.com; dkim=fail header.i=@paypal.com; spf=softfail smtp.mailfrom=x@evil.example; dmarc=fail (p=REJECT) header.from=paypal.com";
    const NO_DMARC_DKIM_ALIGNED: &str = "mx.google.com; dkim=pass header.i=@news.shop.example header.s=s1; spf=neutral smtp.mailfrom=b@bounce.esp.example";
    const NO_DMARC_FOREIGN_DKIM: &str =
        "mx.google.com; dkim=pass header.i=@esp.example; spf=neutral smtp.mailfrom=b@esp.example";
    /// Ann's own provider signs her mail; Gmail got it straight from there.
    const DIRECT: &str = "mx.google.com; dkim=pass header.i=@mail.example; spf=pass (google.com: domain of ann@mail.example designates 192.0.2.1 as permitted sender) smtp.mailfrom=ann@mail.example; dmarc=pass (p=NONE sp=QUARANTINE dis=NONE) header.from=mail.example";
    /// The same mail sent to an old address that forwards to Gmail: the relay altered
    /// the body and sends from its own servers.
    const FORWARDED: &str = "mx.google.com; spf=softfail (google.com: domain of transitioning ann@mail.example does not designate 198.51.100.7 as permitted sender) smtp.mail=ann@mail.example; dkim=neutral (body hash did not verify) header.i=@; dmarc=fail (p=NONE dis=NONE) header.from=mail.example";
    /// A forwarder that re-signs the copy with its own domain.
    const FORWARDED_RESIGNED: &str = "mx.google.com; dkim=pass header.i=@relay.example; dkim=fail header.i=@mail.example; spf=softfail (google.com: domain of transitioning ann@mail.example does not designate 198.51.100.7 as permitted sender) smtp.mailfrom=ann@mail.example; dmarc=fail (p=NONE sp=NONE dis=NONE) header.from=mail.example";
    /// A charity mailing through a CRM it never aligned with its domain.
    const UNALIGNED_ESP: &str = "mx.google.com; spf=pass (google.com: domain of bounce@crm.example designates 203.0.113.5 as permitted sender) smtp.mailfrom=bounce@crm.example; dmarc=fail (p=NONE sp=NONE dis=NONE) header.from=charity.example";
    const QUARANTINED: &str = "mx.google.com; spf=fail smtp.mailfrom=x@evil.example; dmarc=fail (p=QUARANTINE sp=QUARANTINE dis=QUARANTINE) header.from=bank.example";
    const SUBDOMAIN_POLICY: &str = "mx.google.com; spf=softfail smtp.mailfrom=x@evil.example; dmarc=fail (p=NONE sp=REJECT dis=REJECT) header.from=alerts.bank.example";
    const NO_DMARC_HARD_FAILS: &str =
        "mx.google.com; dkim=fail header.i=@shop.example; spf=fail smtp.mailfrom=x@shop.example";
    const NO_DMARC_SOFT_FAILS: &str = "mx.google.com; dkim=fail header.i=@shop.example; spf=softfail smtp.mailfrom=x@shop.example";
    const NO_DMARC_FOREIGN_DKIM_FAIL: &str =
        "mx.google.com; dkim=fail header.i=@relay.example; spf=fail smtp.mailfrom=x@shop.example";

    #[rstest]
    #[case::dmarc_pass(vec![AWS], "no-reply@amazonaws.com", "pass")]
    #[case::spoofed_dmarc_fail(vec![SPOOF], "service@paypal.com", "fail")]
    #[case::aligned_dkim_without_dmarc(vec![NO_DMARC_DKIM_ALIGNED], "hello@shop.example", "pass")]
    #[case::foreign_dkim_is_inconclusive(vec![NO_DMARC_FOREIGN_DKIM], "hello@shop.example", "none")]
    #[case::forged_header_is_ignored(vec!["evil.example; dmarc=pass header.from=paypal.com"], "service@paypal.com", "none")]
    #[case::gmail_header_wins_over_a_forged_one(vec!["evil.example; dmarc=pass", SPOOF], "service@paypal.com", "fail")]
    #[case::no_header(vec![], "a@b.example", "none")]
    #[case::sender_mailing_directly(vec![DIRECT], "ann@mail.example", "pass")]
    #[case::same_sender_forwarded(vec![FORWARDED], "ann@mail.example", "none")]
    #[case::same_sender_forwarded_and_resigned(vec![FORWARDED_RESIGNED], "ann@mail.example", "none")]
    #[case::unaligned_mailing_tool_without_policy(vec![UNALIGNED_ESP], "news@charity.example", "none")]
    #[case::failure_under_quarantine_policy(vec![QUARANTINED], "alerts@bank.example", "fail")]
    #[case::failure_under_subdomain_reject_policy(vec![SUBDOMAIN_POLICY], "x@alerts.bank.example", "fail")]
    #[case::hard_failures_without_dmarc(vec![NO_DMARC_HARD_FAILS], "x@shop.example", "fail")]
    #[case::soft_spf_failure_without_dmarc(vec![NO_DMARC_SOFT_FAILS], "x@shop.example", "none")]
    #[case::foreign_signature_failure_without_dmarc(vec![NO_DMARC_FOREIGN_DKIM_FAIL], "x@shop.example", "none")]
    fn verdicts(#[case] results: Vec<&str>, #[case] from: &str, #[case] want: &str) {
        let r: Vec<String> = results.into_iter().map(String::from).collect();
        assert_eq!(verdict(&r, from), want);
    }

    #[rstest]
    #[case::jira_site_impersonating_a_bank(
        "jiradnb@haalandlovers001.atlassian.net",
        Some("shared")
    )]
    #[case::atlassian_itself("no-reply@atlassian.net", Some("pass"))]
    #[case::ordinary_domain("no-reply@amazonaws.com", Some("pass"))]
    #[case::failure_stays_failure("x@shop.myshopify.com", Some("fail"))]
    fn shared_platforms_do_not_vouch(#[case] email: &str, #[case] want: Option<&str>) {
        let stored = if want == Some("fail") { "fail" } else { "pass" };
        assert_eq!(shown(Some(stored), "Support", email).as_deref(), want);
    }

    #[rstest]
    #[case::github_copycat("GitHub", "noreply@noreply-github.co", "pass", "lookalike")]
    #[case::slack_copycat("Slack", "noreply@slack-app.cloud", "pass", "lookalike")]
    #[case::brand_first_copycat("OpenAI", "noreply@openai-noreply.com", "none", "lookalike")]
    #[case::country_suffix_copycat("PayPal", "x@secure-paypal.co.uk", "pass", "lookalike")]
    #[case::real_github("GitHub", "noreply@github.com", "pass", "pass")]
    #[case::hyphenated_brand("SNCF Connect", "info@mail.sncf-connect.com", "pass", "pass")]
    #[case::trade_after_a_name("Mélanie d'Ithaque", "m@ithaque-renovation.fr", "pass", "pass")]
    #[case::filler_in_the_name("Bouygues Telecom Service", "x@service-bouygues.fr", "pass", "pass")]
    #[case::address_as_name("licences@ffvl.fr", "postmaster@ml-ffvl.fr", "pass", "pass")]
    #[case::failure_stays_failure("GitHub", "noreply@noreply-github.co", "fail", "fail")]
    fn copycat_domains_do_not_vouch(
        #[case] name: &str,
        #[case] email: &str,
        #[case] stored: &str,
        #[case] want: &str,
    ) {
        assert_eq!(shown(Some(stored), name, email).as_deref(), Some(want));
    }

    fn github_history() -> Vec<(String, i64)> {
        vec![
            ("github.com".into(), 56),
            ("email.github.com".into(), 7),
            ("noreply-github.co".into(), 2),
        ]
    }

    #[rstest]
    #[case::copycat_domain("noreply@noreply-github.co", Some("github.com"))]
    #[case::typosquat_seen_once("noreply@githubb.com", Some("github.com"))]
    #[case::real_domain("noreply@github.com", None)]
    #[case::real_subdomain("hello@email.github.com", None)]
    fn names_have_a_usual_domain(#[case] email: &str, #[case] want: Option<&str>) {
        assert_eq!(usual_domain(&github_history(), email).as_deref(), want);
    }

    #[rstest]
    #[case::too_little_history(vec![("slack.com".into(), 2), ("slacks.app".into(), 1)])]
    #[case::two_regular_domains(vec![("work.example".into(), 9), ("gmail.com".into(), 4)])]
    fn no_usual_domain_without_a_clear_habit(#[case] counts: Vec<(String, i64)>) {
        let email = counts.last().map(|(d, _)| format!("x@{d}")).unwrap();
        assert_eq!(usual_domain(&counts, &email), None);
    }

    #[rstest]
    fn folded_headers_are_read_whole() {
        let raw = b"From: a@b.example\r\nAuthentication-Results: mx.google.com;\r\n       dmarc=pass header.from=b.example\r\nSubject: hi\r\n\r\nbody Authentication-Results: fake";
        assert_eq!(
            results_from_raw(raw),
            vec!["mx.google.com; dmarc=pass header.from=b.example".to_string()]
        );
    }
}
