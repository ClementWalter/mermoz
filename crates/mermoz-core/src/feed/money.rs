//! Finds the amounts a thread mentions ("126,00 €", "$84.20", "1 275 EUR"). Jev picks the
//! one to pay, and the card shows that span exactly as written.

use regex::Regex;
use std::sync::LazyLock;

/// Bills rarely carry more; past this the list is noise (order lines, prices in ads).
pub const MAX_CANDIDATES: usize = 6;

/// A number with thousands separators (space, no-break space, dot, comma) and decimals.
const NUMBER: &str = r"\d{1,3}(?:[ \u{a0}\u{202f}.,]?\d{3})*(?:[.,]\d{1,2})?";

static AMOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)(?:[€$£]\s?{NUMBER}|\b{NUMBER}\s?(?:€|euros?\b|eur\b|usd\b|gbp\b|\$|£|chf\b))"
    ))
    .unwrap()
});

/// Every amount in order of appearance, each written form once.
pub fn candidates(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for m in AMOUNT.find_iter(text) {
        let span = m.as_str().trim().to_string();
        if !out.contains(&span) {
            out.push(span);
        }
        if out.len() == MAX_CANDIDATES {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::euro_after("Montant : 126,00 € à régler", "126,00 €")]
    #[case::euro_before("Total €84.20 due", "€84.20")]
    #[case::dollar("You owe $1,315.50 this month", "$1,315.50")]
    #[case::spaced_thousands("loyer de 1 275 € par mois", "1 275 €")]
    #[case::code("Montant 42 EUR", "42 EUR")]
    #[case::word("pour 30 euros", "30 euros")]
    fn finds_the_amount(#[case] text: &str, #[case] want: &str) {
        assert_eq!(candidates(text).first().map(String::as_str), Some(want));
    }

    #[rstest]
    #[case::plain_number("Invoice 2026-10-4471")]
    #[case::percent("prices are down 1.2% over a year")]
    #[case::no_amount("Merci beaucoup")]
    fn finds_no_amount(#[case] text: &str) {
        assert!(candidates(text).is_empty());
    }

    #[rstest]
    fn each_written_amount_once() {
        assert_eq!(candidates("126,00 € puis 126,00 €").len(), 1);
    }

    #[rstest]
    fn keeps_order_of_appearance() {
        assert_eq!(candidates("1 240 € → 1 275 €")[1], "1 275 €");
    }
}
