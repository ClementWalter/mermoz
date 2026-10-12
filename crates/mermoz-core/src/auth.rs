//! Sender verification from the `Authentication-Results` header Gmail adds on receipt.
//!
//! Only Gmail's own header (authserv-id `mx.google.com`) is trusted: any other copy
//! can be written by the sender. A message is "pass" when DMARC passes, or when DKIM
//! or SPF pass for the From domain; "fail" when DMARC fails or both DKIM and SPF fail;
//! "none" otherwise (nothing conclusive, e.g. old mail or forwarders). A pass from a
//! subdomain of a platform where anyone can open an account (a Jira site, a shop, a
//! hosted page) is "shared": the domain is real, but it vouches for nobody.

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

/// A stored verdict as shown: a pass from a shared platform reads as "shared".
pub fn shown(verdict: Option<&str>, email: &str) -> Option<String> {
    verdict.map(|v| {
        if v == "pass" && is_shared_platform(email) {
            "shared".into()
        } else {
            v.into()
        }
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
    let (mut dmarc, mut dkim_pass, mut dkim_fail, mut spf_pass, mut spf_fail) =
        (None, false, false, false, false);
    for clause in r.split(';').skip(1) {
        let words: Vec<&str> = clause.split_whitespace().collect();
        let Some((method, result)) = words.first().and_then(|w| w.split_once('=')) else {
            continue;
        };
        let prop = |key: &str| {
            words
                .iter()
                .find_map(|w| w.strip_prefix(key))
                .map(|v| {
                    v.trim_start_matches('@')
                        .trim_matches(|c| c == '"' || c == ')' || c == '(')
                        .to_lowercase()
                })
                .unwrap_or_default()
        };
        let result = result.to_lowercase();
        match method.to_lowercase().as_str() {
            "dmarc" => dmarc = Some(result),
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
                } else if result == "fail" {
                    dkim_fail = true;
                }
            }
            "spf" => {
                let d = domain_of(&prop("smtp.mailfrom="));
                if result == "pass" && aligned(&d, &from) {
                    spf_pass = true;
                } else if matches!(result.as_str(), "fail" | "softfail") {
                    spf_fail = true;
                }
            }
            _ => {}
        }
    }
    match dmarc.as_deref() {
        Some("pass") => "pass",
        Some("fail") => "fail",
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

    #[rstest]
    #[case::dmarc_pass(vec![AWS], "no-reply@amazonaws.com", "pass")]
    #[case::spoofed_dmarc_fail(vec![SPOOF], "service@paypal.com", "fail")]
    #[case::aligned_dkim_without_dmarc(vec![NO_DMARC_DKIM_ALIGNED], "hello@shop.example", "pass")]
    #[case::foreign_dkim_is_inconclusive(vec![NO_DMARC_FOREIGN_DKIM], "hello@shop.example", "none")]
    #[case::forged_header_is_ignored(vec!["evil.example; dmarc=pass header.from=paypal.com"], "service@paypal.com", "none")]
    #[case::gmail_header_wins_over_a_forged_one(vec!["evil.example; dmarc=pass", SPOOF], "service@paypal.com", "fail")]
    #[case::no_header(vec![], "a@b.example", "none")]
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
        assert_eq!(shown(Some(stored), email).as_deref(), want);
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
