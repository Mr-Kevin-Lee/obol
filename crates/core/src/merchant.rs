//! Merchant keys (spec §11.4, D44): one canonical name per merchant, so
//! every way a bank writes "Amazon" lands on the same category rule.
//!
//! **Contract (task 111b), implemented by task 113.** `merchant_key`:
//! uppercases, strips payment-processor prefixes (`SQ *`, `TST*`,
//! `PAYPAL *`, `POS `…), store numbers, card last-4 digits, phone numbers,
//! URLs and trailing city/state/zip, collapses whitespace, then applies a
//! small alias table (`AMZN` → `AMAZON`). Two properties other code relies
//! on, which task 113 must keep:
//!
//! 1. **Idempotent**: `merchant_key(&merchant_key(x)) == merchant_key(x)`.
//! 2. **A bare uppercase merchant name is already a key**: `merchant_key("AMAZON")
//!    == "AMAZON"`, `merchant_key("UBER EATS") == "UBER EATS"`. The
//!    categorize and plan streams test against canonical names like these
//!    before this module's real implementation lands.
//!
//! The first word is never cut, so names like `7-ELEVEN` and `76` survive;
//! apostrophes are dropped and a lone `S` joins the word before it, so
//! `TRADER JOE'S` and `TRADER JOE S` both become `TRADER JOES`. `&` stays
//! (`AT&T`, `PG&E`).
//!
//! **Known limitation:** a city is only recognized from padding before it,
//! a store number or phone before it, or an alias. Otherwise it stays in
//! the key (`JOES DINER PORTLAND OR` → `JOES DINER PORTLAND`), which is
//! still the same key every time that merchant posts.

/// US state and DC postal codes, for dropping a trailing location.
const STATES: &[&str] = &[
    "AL", "AK", "AZ", "AR", "CA", "CO", "CT", "DE", "DC", "FL", "GA", "HI", "ID", "IL", "IN", "IA",
    "KS", "KY", "LA", "ME", "MD", "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH", "NJ", "NM",
    "NY", "NC", "ND", "OH", "OK", "OR", "PA", "RI", "SC", "SD", "TN", "TX", "UT", "VT", "VA", "WA",
    "WV", "WI", "WY",
];

/// Processors that write `NAME *merchant`.
const STAR_PREFIXES: &[&str] = &["SQ", "TST", "PAYPAL", "PP", "SP", "IN"];

/// Card-network words before the merchant; longest first, so `POS DEBIT`
/// wins over `POS`.
const WORD_PREFIXES: &[&str] = &[
    "RECURRING PAYMENT AUTHORIZED ON",
    "PURCHASE AUTHORIZED ON",
    "DEBIT CARD PURCHASE",
    "POS PURCHASE",
    "POS DEBIT",
    "DEBIT PURCHASE",
    "CHECKCARD",
    "POS",
];

const TLDS: &[&str] = &["COM", "NET", "ORG", "IO", "CO", "TV"];

/// A key's leading words → its canonical key. The longest match wins, so
/// `UBER EATS` isn't swallowed by `UBER`. Aliases only map variants onto a
/// canonical name; they never merge two canonical names.
const ALIASES: &[(&str, &str)] = &[
    ("AMAZON", "AMAZON"),
    ("AMZN", "AMAZON"),
    ("AT&T", "AT&T"),
    ("COSTCO", "COSTCO"),
    ("CVS/PHARMACY", "CVS"),
    ("LYFT", "LYFT"),
    ("NETFLIX", "NETFLIX"),
    ("PAYMENT THANK YOU", "PAYMENT THANK YOU"),
    ("PAYMENT THANK YOU-MOBILE", "PAYMENT THANK YOU"),
    ("SHELL", "SHELL"),
    ("SPOTIFY", "SPOTIFY"),
    ("STARBUCKS", "STARBUCKS"),
    ("THE HOME DEPOT", "HOME DEPOT"),
    ("UBER", "UBER"),
    ("UBER EATS", "UBER EATS"),
    ("VERIZON", "VERIZON"),
    ("WAL-MART", "WALMART"),
    ("WHOLEFDS", "WHOLE FOODS"),
];

/// Words whose brand spelling isn't title case.
const DISPLAY_OVERRIDES: &[(&str, &str)] = &[
    ("AMC", "AMC"),
    ("AT&T", "AT&T"),
    ("CVS", "CVS"),
    ("DOORDASH", "DoorDash"),
    ("IKEA", "IKEA"),
    ("MCDONALDS", "McDonald's"),
    ("PG&E", "PG&E"),
];

pub fn merchant_key(description: &str) -> String {
    // A pass only removes noise or settles on an alias, so repeating it
    // until nothing changes makes the key idempotent by construction.
    let mut key = normalize_once(description);
    loop {
        let next = normalize_once(&key);
        if next == key {
            return key;
        }
        key = next;
    }
}

/// A merchant key as people would write it: `UBER EATS` → `Uber Eats`.
pub fn display_name(merchant_key: &str) -> String {
    merchant_key
        .split(' ')
        .map(
            |word| match DISPLAY_OVERRIDES.iter().find(|(key, _)| *key == word) {
                Some((_, brand)) => brand.to_string(),
                None => word
                    .split('-')
                    .map(capitalize)
                    .collect::<Vec<_>>()
                    .join("-"),
            },
        )
        .collect::<Vec<_>>()
        .join(" ")
}

fn capitalize(part: &str) -> String {
    let mut chars = part.chars();
    chars.next().map_or_else(String::new, |first| {
        first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect()
    })
}

fn normalize_once(description: &str) -> String {
    let upper = description.to_uppercase();
    let text = strip_padded_location(&upper)
        .replace(['\'', '’'], "")
        .replace(',', " ")
        .replace('*', " * ");

    let mut tokens: Vec<String> = Vec::new();
    for token in text.split_whitespace().filter(|token| *token != "-") {
        match tokens.last_mut() {
            // How some banks write `JOE'S`.
            Some(previous) if token == "S" => previous.push('S'),
            _ => tokens.push(token.to_string()),
        }
    }

    while strip_prefix(&mut tokens) {}
    let next = |i: usize| tokens.get(i + 1).map(String::as_str);
    if let Some(cut) = (1..tokens.len()).find(|&i| is_noise(&tokens[i], next(i))) {
        tokens.truncate(cut);
    }
    tokens.retain(|token| token != "*");
    if tokens.last().is_some_and(|token| is_state(token)) {
        tokens.pop();
    }

    let Some(first) = tokens.first_mut() else {
        // Nothing but noise: keep the description rather than an empty key.
        return upper.split_whitespace().collect::<Vec<_>>().join(" ");
    };
    if let Some(name) = domain_name(first).map(str::to_string) {
        *first = name;
    }
    apply_alias(&tokens)
}

/// Fixed-width descriptions pad between the name and the city
/// (`COFFEE   OAKLAND      CA`), so a padded tail ending in a state is
/// the location.
fn strip_padded_location(text: &str) -> &str {
    let text = text.trim();
    match text.split_once("  ") {
        Some((name, location)) if ends_with_state(location) => name,
        _ => text,
    }
}

fn ends_with_state(location: &str) -> bool {
    let mut words = location.split_whitespace().rev();
    let last = words.next();
    let state = if last.is_some_and(is_zip) {
        words.next()
    } else {
        last
    };
    state.is_some_and(is_state)
}

fn is_state(word: &str) -> bool {
    STATES.contains(&word)
}

fn is_zip(word: &str) -> bool {
    word.len() == 5 && word.bytes().all(|b| b.is_ascii_digit())
}

/// Removes one processor or card-network prefix; false if there was none.
fn strip_prefix(tokens: &mut Vec<String>) -> bool {
    if tokens.len() > 1 && tokens[1] == "*" && STAR_PREFIXES.contains(&tokens[0].as_str()) {
        tokens.drain(..2);
        return true;
    }
    let Some(prefix) = WORD_PREFIXES
        .iter()
        .find(|prefix| starts_with_words(tokens, prefix.split(' ')))
    else {
        return false;
    };
    tokens.drain(..prefix.split(' ').count());
    // A transaction date or masked card number can follow the prefix.
    if tokens
        .first()
        .is_some_and(|token| is_date(token) || is_card_mask(token))
    {
        tokens.remove(0);
    }
    true
}

fn starts_with_words<'a>(tokens: &[String], words: impl IntoIterator<Item = &'a str>) -> bool {
    let mut tokens = tokens.iter();
    words
        .into_iter()
        .all(|word| tokens.next().is_some_and(|token| token == word))
}

/// Where the name ends: everything from this token on is a store number,
/// phone, reference code, URL or location.
fn is_noise(token: &str, next: Option<&str>) -> bool {
    let digits = token.bytes().filter(u8::is_ascii_digit).count();
    match token {
        // Before a reference code (`MKTPL*AB12CD`); a word after it is
        // part of the name (`UBER *EATS`).
        "*" => next.is_some_and(|next| next.bytes().any(|b| b.is_ascii_digit())),
        "CARD" => next.is_some_and(|next| next.parse::<u64>().is_ok()),
        "PPD" | "CCD" | "WEB" => next == Some("ID:"),
        _ => {
            token.starts_with('#')
                || (digits >= 3
                    && token
                        .bytes()
                        .all(|b| b.is_ascii_digit() || b"-()./".contains(&b)))
                || is_store_code(token)
                || is_card_mask(token)
                || domain_name(token).is_some()
        }
    }
}

/// `1024` or `10/24`.
fn is_date(token: &str) -> bool {
    token.bytes().filter(u8::is_ascii_digit).count() == 4
        && token.bytes().all(|b| b.is_ascii_digit() || b == b'/')
}

/// `XXXX1234`.
fn is_card_mask(token: &str) -> bool {
    token.starts_with("XX")
        && token
            .trim_start_matches('X')
            .bytes()
            .all(|b| b.is_ascii_digit())
}

/// A letter-led store number: `F12345`, `T-1234`.
fn is_store_code(token: &str) -> bool {
    let mut chars = token.chars();
    let led_by_letter = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    let rest = chars.as_str();
    let number = rest.strip_prefix('-').unwrap_or(rest);
    led_by_letter && number.len() >= 3 && number.bytes().all(|b| b.is_ascii_digit())
}

/// The name in a domain: `NETFLIX.COM` → `NETFLIX`, `AMZN.COM/BILL` → `AMZN`.
fn domain_name(token: &str) -> Option<&str> {
    let host = token.split('/').next().unwrap_or(token);
    let mut labels = host.rsplit('.');
    let tld = labels.next()?;
    let name = labels.next()?;
    (TLDS.contains(&tld) && !name.is_empty()).then_some(name)
}

fn apply_alias(tokens: &[String]) -> String {
    ALIASES
        .iter()
        .filter(|(variant, _)| starts_with_words(tokens, variant.split(' ')))
        .max_by_key(|(variant, _)| variant.len())
        .map_or_else(|| tokens.join(" "), |(_, canonical)| canonical.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Realistic US bank and card descriptions and the key each must reach.
    const EXAMPLES: &[(&str, &str)] = &[
        ("AMZN Mktp US*2K4AB1234", "AMAZON"),
        ("Amazon.com*2K4AB1234 Amzn.com/bill WA", "AMAZON"),
        ("AMAZON MKTPL*AB12CD", "AMAZON"),
        ("AMZN Digital*1A2B3C4D", "AMAZON"),
        ("SQ *BLUE BOTTLE COFFEE", "BLUE BOTTLE COFFEE"),
        ("SQ *BLUE BOTTLE COFFEE   OAKLAND      CA", "BLUE BOTTLE COFFEE"),
        ("TST* CORNER BODEGA", "CORNER BODEGA"),
        ("PAYPAL *SPOTIFY", "SPOTIFY"),
        ("PP*NETFLIX", "NETFLIX"),
        ("STARBUCKS STORE 12345 SEATTLE WA", "STARBUCKS"),
        ("SHELL OIL 57444212400 SAN JOSE CA 95123", "SHELL"),
        ("CHEVRON 0204512 SAN RAMON CA", "CHEVRON"),
        ("COSTCO WHSE #0123 ISSAQUAH WA", "COSTCO"),
        ("COSTCO WHSE ISSAQUAH WA", "COSTCO"),
        ("WHOLEFDS MKT 10234", "WHOLE FOODS"),
        ("SAFEWAY #1234 OAKLAND CA 94611", "SAFEWAY"),
        ("NETFLIX.COM 866-579-7172 CA", "NETFLIX"),
        ("UBER *TRIP HELP.UBER.COM CA", "UBER"),
        ("UBER *EATS", "UBER EATS"),
        ("LYFT   *1 RIDE 10-24", "LYFT"),
        ("Payment Thank You - Web", "PAYMENT THANK YOU"),
        ("Payment Thank You-Mobile", "PAYMENT THANK YOU"),
        ("AUTOPAY 241024", "AUTOPAY"),
        ("ACME PAYROLL PPD ID: 9111111101", "ACME PAYROLL"),
        (
            "PURCHASE AUTHORIZED ON 10/24 STARBUCKS STORE 12345 SEATTLE WA S386297123456789 CARD 1234",
            "STARBUCKS",
        ),
        ("POS DEBIT 1024 TRADER JOE'S #552 SEATTLE WA", "TRADER JOES"),
        ("TRADER JOE S #552 SEATTLE WA", "TRADER JOES"),
        ("DEBIT CARD PURCHASE - XXXX1234 IN-N-OUT BURGER 123", "IN-N-OUT BURGER"),
        ("CHECKCARD 1024 CHIPOTLE 1234 SEATTLE WA", "CHIPOTLE"),
        ("7-ELEVEN 41234 DALLAS TX", "7-ELEVEN"),
        ("MCDONALD'S F12345 CHICAGO IL", "MCDONALDS"),
        ("CVS/PHARMACY #01234 SAN FRANCISCO CA", "CVS"),
        ("THE HOME DEPOT #6789 SEATTLE WA", "HOME DEPOT"),
        ("WAL-MART #1234", "WALMART"),
        ("AT&T *BILL PAYMENT 800-331-0500 TX", "AT&T"),
        ("COMCAST (800) 266-2278 CA", "COMCAST"),
        ("VERIZON WRLS 855.123.4567", "VERIZON"),
        ("AIRBNB * HMABC12345", "AIRBNB"),
        ("Spotify USA 877-778-1161 NY", "SPOTIFY"),
        ("  uber   eats  ", "UBER EATS"),
        ("  SQ *TACOS EL GORDO  LOS ANGELES     CA 90012", "TACOS EL GORDO"),
        ("TARGET T-1234 SEATTLE WA", "TARGET"),
        ("POS SQ *BLUE BOTTLE COFFEE", "BLUE BOTTLE COFFEE"),
        ("RECURRING PAYMENT AUTHORIZED ON 10/01 HULU CARD 5678", "HULU"),
        ("CORNER BODEGA XXXX1234", "CORNER BODEGA"),
        ("CORNER BODEGA, CA", "CORNER BODEGA"),
        ("BLUE APRON BLUEAPRON.COM NY", "BLUE APRON"),
        ("AMZN.COM/BILL WA", "AMAZON"),
        ("TST* 1000 DEGREES PIZZA", "1000 DEGREES PIZZA"),
    ];

    /// Names other streams use as keys: each must already be its own key.
    const CANONICAL: &[&str] = &[
        "AMAZON",
        "TARGET",
        "WALMART",
        "BEST BUY",
        "HOME DEPOT",
        "LOWES",
        "IKEA",
        "COSTCO",
        "SAFEWAY",
        "WHOLE FOODS",
        "TRADER JOES",
        "KROGER",
        "STARBUCKS",
        "CHIPOTLE",
        "MCDONALDS",
        "UBER EATS",
        "DOORDASH",
        "GRUBHUB",
        "UBER",
        "LYFT",
        "SHELL",
        "CHEVRON",
        "NETFLIX",
        "SPOTIFY",
        "HULU",
        "CVS",
        "WALGREENS",
        "COMCAST",
        "VERIZON",
        "XFINITY",
        "PLANET FITNESS",
        "PETCO",
        "CHEWY",
        "DELTA AIR",
        "UNITED AIRLINES",
        "AIRBNB",
        "AT&T",
        "PG&E",
        "AUTOPAY",
        "PAYMENT THANK YOU",
        "ONLINE TRANSFER",
        "TRANSFER TO",
        "TRANSFER FROM",
        "CREDIT CARD PAYMENT",
        "PAYROLL",
        "DIRECT DEP",
        "ACME PAYROLL",
        "RENT PORTAL",
        "PAYMENT RECEIVED",
        "CHASE CARD AUTOPAY",
        "BLUE BOTTLE COFFEE",
        "TAX REFUND",
        "AMC THEATRES",
        "CORNER BODEGA",
        "CITY WATER",
        "STATE FARM",
        "GYM CLUB",
        "APPLE CARD PAYMENT",
    ];

    fn mismatches(
        cases: impl IntoIterator<Item = (String, String)>,
    ) -> Vec<(String, String, String)> {
        cases
            .into_iter()
            .map(|(input, expected)| {
                let actual = merchant_key(&input);
                (input, expected, actual)
            })
            .filter(|(_, expected, actual)| expected != actual)
            .collect()
    }

    #[test]
    fn should_reduce_bank_descriptions_to_merchant_keys() {
        let cases = EXAMPLES.iter().map(|(i, e)| (i.to_string(), e.to_string()));

        assert_eq!(mismatches(cases), []);
    }

    #[test]
    fn should_keep_every_canonical_name_as_its_own_key() {
        let cases = CANONICAL
            .iter()
            .map(|name| (name.to_string(), name.to_string()));

        assert_eq!(mismatches(cases), []);
    }

    #[test]
    fn should_be_idempotent_over_every_expected_key() {
        let cases = EXAMPLES.iter().map(|(_, e)| (e.to_string(), e.to_string()));

        assert_eq!(mismatches(cases), []);
    }

    #[test]
    fn should_be_idempotent_over_every_raw_description() {
        let cases = EXAMPLES.iter().map(|(i, _)| {
            let key = merchant_key(i);
            (key.clone(), key)
        });

        assert_eq!(mismatches(cases), []);
    }

    #[test]
    fn should_be_idempotent_when_dropping_a_state_exposes_another() {
        let key = merchant_key("CORNER BODEGA OR CA");

        assert_eq!(merchant_key(&key), key);
    }

    #[test]
    fn should_strip_stacked_processor_prefixes() {
        assert_eq!(
            merchant_key("PAYPAL *SQ *BLUE BOTTLE COFFEE"),
            "BLUE BOTTLE COFFEE"
        );
    }

    #[test]
    fn should_keep_a_city_it_cannot_tell_from_the_name() {
        // The accepted limitation: with no padding, store number or alias
        // to mark where the name ends, the city stays in the key.
        assert_eq!(
            merchant_key("JOES DINER PORTLAND OR"),
            "JOES DINER PORTLAND"
        );
    }

    #[test]
    fn should_keep_a_name_that_is_only_a_state_code() {
        assert_eq!(merchant_key("CA"), "CA");
    }

    #[test]
    fn should_keep_the_description_when_stripping_would_leave_nothing() {
        assert_eq!(merchant_key("sq *"), "SQ *");
    }

    #[test]
    fn should_keep_digits_inside_the_first_word() {
        assert_eq!(merchant_key("76 #0123 FRESNO CA"), "76");
    }

    #[test]
    fn should_keep_short_numbers_inside_a_name() {
        assert_eq!(merchant_key("FOREVER 21 #123"), "FOREVER 21");
    }

    #[test]
    fn should_cut_at_a_letter_led_store_number_of_three_digits() {
        assert_eq!(merchant_key("CORNER BODEGA T-123"), "CORNER BODEGA");
    }

    #[test]
    fn should_keep_letter_led_codes_that_are_not_store_numbers() {
        assert_eq!(
            ["CAFE B12", "CAFE A1B2C3"].map(merchant_key),
            ["CAFE B12", "CAFE A1B2C3"]
        );
    }

    #[test]
    fn should_keep_an_abbreviation_with_a_dot() {
        assert_eq!(merchant_key("DR. SMITH DDS"), "DR. SMITH DDS");
    }

    #[test]
    fn should_keep_a_bare_top_level_domain() {
        assert_eq!(merchant_key(".NET"), ".NET");
    }

    #[test]
    fn should_title_case_each_word_for_display() {
        assert_eq!(display_name("UBER EATS"), "Uber Eats");
    }

    #[test]
    fn should_title_case_each_hyphenated_part_for_display() {
        assert_eq!(display_name("IN-N-OUT BURGER"), "In-N-Out Burger");
    }

    #[test]
    fn should_keep_a_leading_digit_for_display() {
        assert_eq!(display_name("7-ELEVEN"), "7-Eleven");
    }

    #[test]
    fn should_use_the_brand_spelling_for_display() {
        let names = [
            "CVS",
            "IKEA",
            "AT&T",
            "PG&E",
            "AMC THEATRES",
            "MCDONALDS",
            "DOORDASH",
        ];

        assert_eq!(
            names.map(display_name),
            [
                "CVS",
                "IKEA",
                "AT&T",
                "PG&E",
                "AMC Theatres",
                "McDonald's",
                "DoorDash"
            ]
        );
    }

    #[test]
    fn should_display_an_empty_key_as_empty() {
        assert_eq!(display_name(""), "");
    }
}
