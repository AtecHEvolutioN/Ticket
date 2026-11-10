use ticket_watch::{
    config::{PageAdapter, Watch},
    model::{Availability, InventoryKind, QuantityKind},
    parser,
};

const PAGE: &str = include_str!("fixtures/ticketmaster-regional.html");
const CZECH: &str = include_str!("fixtures/ticketmaster-regional-cz.html");
const DUTCH: &str = include_str!("fixtures/ticketmaster-regional-nl.html");
const SWEDISH: &str = include_str!("fixtures/ticketmaster-regional-se.html");
const BRITISH: &str = include_str!("fixtures/ticketmaster-regional-uk.html");
const BRITISH_SEATED: &str = include_str!("fixtures/ticketmaster-regional-uk-seated.html");
const GLOBAL_DISPLAY: &str = include_str!("fixtures/ticketmaster-global-display.html");
const ITALIAN_DISPLAY: &str = include_str!("fixtures/ticketmaster-italian-display.html");

fn watch() -> Watch {
    let mut watch = Watch {
        id: "any-selected-event".into(),
        url: "https://www.ticketmaster.pl/event/a-different-artist/123456789".into(),
        ..Watch::default()
    };
    watch.parser.adapter = PageAdapter::Ticketmaster;
    watch
}

fn display_watch(italian: bool) -> Watch {
    let mut watch = watch();
    watch.url = if italian {
        "https://www.ticketmaster.it/biglietti/example-concert/event/abc123def456"
    } else {
        "https://www.ticketmaster.com/example-concert/event/ABCDEF1234567890"
    }
    .into();
    watch
}

#[test]
fn global_price_cards_preserve_different_quotes_without_claiming_tickets_or_stock() {
    let result = parser::parse(&display_watch(false), GLOBAL_DISPLAY, 10);
    assert_eq!(
        result.adapter.as_deref(),
        Some("ticketmaster-global-display-v1")
    );
    assert_eq!(result.availability, Availability::Unknown);
    assert!(result.offers.is_empty());
    assert!(!result.complete);
    assert_eq!(result.ticket_categories.len(), 3);
    assert_eq!(
        result
            .ticket_categories
            .iter()
            .filter(|category| category.inventory_kind == InventoryKind::Resale)
            .count(),
        2
    );
    assert_eq!(
        result
            .ticket_categories
            .iter()
            .filter(|category| category.inventory_kind == InventoryKind::Primary)
            .count(),
        1
    );
    assert!(
        result
            .ticket_categories
            .iter()
            .all(|category| category.selectable_quantity.is_none()
                && category.currency.as_deref() == Some("USD"))
    );
    let mut same_row = result
        .ticket_categories
        .iter()
        .filter(|category| {
            category.section.as_deref() == Some("101") && category.row.as_deref() == Some("4")
        })
        .map(|category| category.price_min_minor.unwrap())
        .collect::<Vec<_>>();
    same_row.sort();
    assert_eq!(same_row, [7550, 8000]);
    assert!(
        result
            .ticket_categories
            .iter()
            .any(|category| category.ticket_type == "Standard Ticket"
                && category.price_text.as_deref() == Some("$95.00"))
    );
    // A duplicate rendered card is not another ticket or another price quote.
    let first = GLOBAL_DISPLAY
        .split("<li role=\"menuitem\"")
        .nth(1)
        .unwrap()
        .split("</li>")
        .next()
        .unwrap();
    let duplicate =
        GLOBAL_DISPLAY.replace("</ul>", &format!("<li role=\"menuitem\"{first}</li></ul>"));
    assert_eq!(
        parser::parse(&display_watch(false), &duplicate, 10).ticket_categories,
        result.ticket_categories
    );
}

#[test]
fn global_display_conflicting_channel_markers_are_unknown_but_ticket_types_are_independent() {
    for (extra, expected) in [
        (
            "<span data-bdd='quick-picks-resale-branding'>Verified Resale Ticket</span>",
            InventoryKind::Unknown,
        ),
        (
            "<span data-bdd='quick-picks-resale-branding' hidden>Verified Resale Ticket</span>",
            InventoryKind::Primary,
        ),
    ] {
        let html = GLOBAL_DISPLAY.replace(
            "<span>Standard Ticket</span>",
            &format!("<span>Standard Ticket</span>{extra}"),
        );
        let result = parser::parse(&display_watch(false), &html, 10);
        assert_eq!(
            result
                .ticket_categories
                .iter()
                .find(|category| category.price_min_minor == Some(9500))
                .unwrap()
                .inventory_kind,
            expected
        );
        assert!(result.offers.is_empty());
        assert_eq!(result.availability, Availability::Unknown);
    }
    for label in [
        "Verified Resale Ticket",
        "Primary / Resale",
        "Original and Resale tickets",
    ] {
        let html = GLOBAL_DISPLAY.replace(
            "<span>Standard Ticket</span>",
            &format!("<span>{label}</span>"),
        );
        let result = parser::parse(&display_watch(false), &html, 10);
        assert_eq!(
            result
                .ticket_categories
                .iter()
                .find(|category| category.price_min_minor == Some(9500))
                .unwrap()
                .inventory_kind,
            InventoryKind::Unknown
        );
    }
    for (extra, expected) in [
        (
            "<span data-bdd='branding-ticket-text'>Standard Ticket</span>",
            InventoryKind::Resale,
        ),
        (
            "<span data-bdd='branding-ticket-text'>Official Platinum</span>",
            InventoryKind::Resale,
        ),
        (
            "<span data-bdd='quick-picks-primary-branding'>Primary</span>",
            InventoryKind::Unknown,
        ),
        (
            "<span data-bdd='quick-picks-primary-branding' hidden>Primary</span>",
            InventoryKind::Resale,
        ),
    ] {
        let html = GLOBAL_DISPLAY.replace(
            "Verified Resale Ticket</span>",
            &format!("Verified Resale Ticket</span>{extra}"),
        );
        let result = parser::parse(&display_watch(false), &html, 10);
        assert!(
            result
                .ticket_categories
                .iter()
                .filter(|category| category.price_min_minor != Some(9500))
                .all(|category| category.inventory_kind == expected)
        );
        assert_eq!(result.ticket_categories.len(), 3);
    }
    let opposite = GLOBAL_DISPLAY.replace("Verified Resale Ticket", "Original tickets");
    assert!(
        parser::parse(&display_watch(false), &opposite, 10)
            .ticket_categories
            .iter()
            .filter(|category| category.price_min_minor != Some(9500))
            .all(|category| category.inventory_kind == InventoryKind::Unknown)
    );
}

#[test]
fn identical_display_quotes_with_different_source_channels_remain_distinct() {
    let html = GLOBAL_DISPLAY.replace("Verified Resale Ticket", "Standard Ticket");
    let first = html
        .split("<li role=\"menuitem\"")
        .nth(1)
        .unwrap()
        .split("</li>")
        .next()
        .unwrap();
    let primary = first
        .replace(
            "quick-picks-list-item-resale",
            "quick-picks-list-item-primary-7",
        )
        .replace("quick-picks-resale-branding", "branding-ticket-text");
    let html = html.replace(
        "</ul>",
        &format!("<li role=\"menuitem\"{primary}</li></ul>"),
    );
    let result = parser::parse(&display_watch(false), &html, 10);
    assert_eq!(result.ticket_categories.len(), 4);
    let same_quote = result
        .ticket_categories
        .iter()
        .filter(|category| category.price_min_minor == Some(7550))
        .collect::<Vec<_>>();
    assert_eq!(same_quote.len(), 2);
    assert!(
        same_quote
            .iter()
            .any(|category| category.inventory_kind == InventoryKind::Primary)
    );
    assert!(
        same_quote
            .iter()
            .any(|category| category.inventory_kind == InventoryKind::Resale)
    );
    assert_ne!(same_quote[0].key, same_quote[1].key);
    assert!(result.offers.is_empty());
}

#[test]
fn global_display_requires_same_event_identity_and_unambiguous_currency() {
    let expected = display_watch(false);
    for page in [
        GLOBAL_DISPLAY.replace("rel=\"canonical\"", "rel=\"unrecognized\""),
        GLOBAL_DISPLAY.replace("href=\"https://www.ticketmaster.com/example-concert/event/ABCDEF1234567890\"", "href=\"#event\""),
        GLOBAL_DISPLAY.replace("href=\"https://www.ticketmaster.com/example-concert/event/ABCDEF1234567890\"", "href=\"&#1;#event\""),
        GLOBAL_DISPLAY.replace("href=\"https://www.ticketmaster.com/example-concert/event/ABCDEF1234567890\"", "href=\"&#1;?language=en-us\""),
        GLOBAL_DISPLAY.replace("ABCDEF1234567890", "1111111111111111"),
        GLOBAL_DISPLAY.replace("</head>", "<link rel='canonical' href='https://www.ticketmaster.com/other/event/1111111111111111'></head>"),
    ] {
        let result = parser::parse(&expected, &page, 10);
        assert!(result.ticket_categories.is_empty());
        assert!(result.offers.is_empty());
    }
    let no_hint = GLOBAL_DISPLAY.replace("\"priceCurrency\":\"USD\",", "");
    let result = parser::parse(&expected, &no_hint, 10);
    assert_eq!(result.ticket_categories.len(), 3);
    assert!(
        result
            .ticket_categories
            .iter()
            .all(|category| category.currency.is_none() && category.price_min_minor.is_none())
    );
    let blank_event = GLOBAL_DISPLAY.replace(
        "\"url\":\"https://www.ticketmaster.com/example-concert/event/ABCDEF1234567890\"",
        "\"url\":\"\"",
    );
    assert!(
        parser::parse(&expected, &blank_event, 10)
            .ticket_categories
            .iter()
            .all(|category| category.currency.is_none() && category.price_min_minor.is_none())
    );
    let contradiction =
        GLOBAL_DISPLAY.replace("\"priceCurrency\":\"USD\"", "\"priceCurrency\":\"EUR\"");
    assert!(
        parser::parse(&expected, &contradiction, 10)
            .ticket_categories
            .iter()
            .all(|category| category.currency.is_none() && category.price_min_minor.is_none())
    );
}

#[test]
fn global_display_currency_can_use_bound_accessible_price_labels_but_not_filter_amounts() {
    const LABELS: &str = "<fieldset><input id='min-price' value='$1'><label for='min-price'><span>Minimum ticket price in US Dollars</span></label><input id='max-price' value='$9999'><label for='max-price'><span>Maximum ticket price in US Dollars</span></label></fieldset>";
    let no_metadata_currency = GLOBAL_DISPLAY.replace("\"priceCurrency\":\"USD\",", "");
    let html = no_metadata_currency.replace("<body>", &format!("<body>{LABELS}"));
    let result = parser::parse(&display_watch(false), &html, 10);
    assert_eq!(result.ticket_categories.len(), 3);
    assert!(
        result
            .ticket_categories
            .iter()
            .all(|category| category.currency.as_deref() == Some("USD")
                && category.price_min_minor.is_some_and(|price| price >= 7550))
    );
    for changed in [
        html.replace(
            "Maximum ticket price in US Dollars",
            "Maximum ticket price in Canadian Dollars",
        ),
        html.replace("for='min-price'", "for='missing-input'"),
        html.replace("id='min-price'", "id='min-price' disabled"),
        html.replace("<fieldset>", "<fieldset hidden>"),
        html.replace(
            "</fieldset>",
            &format!("{}</fieldset>", "<input id='unrelated'>".repeat(257)),
        ),
        html.replace(
            "<input id='min-price' value='$1'>",
            "<input id='min-price' value='$1'><input id='min-price' value='$1'>",
        ),
        html.replace(
            "\"price\":\"1.00\"",
            "\"priceCurrency\":\"EUR\",\"price\":\"1.00\"",
        ),
    ] {
        assert!(
            parser::parse(&display_watch(false), &changed, 10)
                .ticket_categories
                .iter()
                .all(|category| category.currency.is_none() && category.price_min_minor.is_none())
        );
    }
}

#[test]
fn global_display_omits_locked_hidden_disabled_and_malformed_price_cards() {
    for page in [
        GLOBAL_DISPLAY.replace(
            "aria-haspopup=\"dialog\"",
            "aria-haspopup=\"dialog\" hidden",
        ),
        GLOBAL_DISPLAY.replace(
            "aria-haspopup=\"dialog\"",
            "aria-haspopup=\"dialog\" aria-disabled=\"true\"",
        ),
        GLOBAL_DISPLAY.replace(
            "data-analytics=\"offer-card\">",
            "data-analytics=\"offer-card\"><span data-bdd='icon-lock'></span>",
        ),
        GLOBAL_DISPLAY.replace(
            "data-bdd=\"quick-pick-price-button\"",
            "data-bdd=\"changed-price\"",
        ),
        GLOBAL_DISPLAY.replace("data-price=\"$", "data-price=\"EUR "),
        GLOBAL_DISPLAY.replace(
            "</li>",
            "<span data-bdd='quick-pick-price-button'>$10.00</span></li>",
        ),
    ] {
        let result = parser::parse(&display_watch(false), &page, 10);
        assert_eq!(result.availability, Availability::Unknown);
        assert!(result.ticket_categories.is_empty());
        assert!(result.offers.is_empty());
    }
}

#[test]
fn italian_display_preserves_visible_fee_text_and_ranges_but_skips_unavailable_rows() {
    let result = parser::parse(&display_watch(true), ITALIAN_DISPLAY, 10);
    assert_eq!(
        result.adapter.as_deref(),
        Some("ticketmaster-italian-display-v1")
    );
    assert_eq!(result.availability, Availability::Unknown);
    assert!(result.offers.is_empty());
    assert!(!result.complete);
    assert_eq!(result.ticket_categories.len(), 2);
    assert!(
        result
            .ticket_categories
            .iter()
            .all(|category| category.inventory_kind == InventoryKind::Unknown)
    );
    assert!(
        result
            .ticket_categories
            .iter()
            .all(|category| category.selectable_quantity.is_none()
                && category.currency.as_deref() == Some("EUR")
                && category.price_text.as_ref().unwrap().ends_with("+ fees"))
    );
    let range = result
        .ticket_categories
        .iter()
        .find(|category| category.ticket_type == "Second Tier")
        .unwrap();
    assert_eq!(
        (range.price_min_minor, range.price_max_minor),
        (Some(7000), Some(10000))
    );
    assert!(
        result
            .ticket_categories
            .iter()
            .any(|category| category.ticket_type.starts_with("Example VIP")
                && category.price_min_minor == Some(15050))
    );
    assert!(
        !result
            .ticket_categories
            .iter()
            .any(|category| category.ticket_type.contains("Unavailable"))
    );
}

#[test]
fn italian_display_requires_reviewed_identity_structure_and_active_rows() {
    for page in [
        ITALIAN_DISPLAY.replace("rel=\"alternate\"", "rel=\"unrecognized\""),
        ITALIAN_DISPLAY.replacen("abc123def456", "wrong123456", 1),
        ITALIAN_DISPLAY.replace("class=\"edpSeatMap\"", "class=\"unrecognized\""),
        ITALIAN_DISPLAY.replace(
            "<div><div class=\"edpSeatMap\"",
            "<div class=\"maxCart\"><div class=\"edpSeatMap\"",
        ),
        ITALIAN_DISPLAY.replace("class=\"ticketInfoDiv\"", "class=\"ticketInfoDiv\" hidden"),
    ] {
        let result = parser::parse(&display_watch(true), &page, 10);
        assert!(result.ticket_categories.is_empty());
        assert!(result.offers.is_empty());
    }
    let foreign = parser::parse(&display_watch(false), ITALIAN_DISPLAY, 10);
    assert!(foreign.ticket_categories.is_empty());
    let reverse = ITALIAN_DISPLAY.replace("from 70.00€ to 100.00€", "from 100.00€ to 70.00€");
    let result = parser::parse(&display_watch(true), &reverse, 10);
    let range = result
        .ticket_categories
        .iter()
        .find(|category| category.ticket_type == "Second Tier")
        .unwrap();
    assert_eq!((range.price_min_minor, range.price_max_minor), (None, None));
}

#[test]
fn same_adapter_extracts_different_events_without_event_specific_selectors() {
    let first = parser::parse(&watch(), PAGE, 10);
    let mut other = watch();
    other.url = "https://www.ticketmaster.pl/event/another-event/987654321".into();
    other.id = "second-choice".into();
    let second = parser::parse(
        &other,
        &PAGE.replace("Example arena show", "A different concert"),
        20,
    );
    assert_eq!(first.offers, second.offers);
    assert_eq!(first.ticket_categories, second.ticket_categories);
    assert_eq!(second.watch_id, "second-choice");
    assert_eq!(first.availability, Availability::Available);
    assert_eq!(first.offers.len(), 2);
    assert!(
        first
            .offers
            .iter()
            .all(|offer| offer.inventory_kind == InventoryKind::Resale)
    );
    assert!(
        first
            .ticket_categories
            .iter()
            .all(|category| category.inventory_kind == InventoryKind::Unknown)
    );
    assert_eq!(first.ticket_categories.len(), 4);
    assert!(!first.complete);
    assert!(
        first
            .offers
            .iter()
            .all(|offer| offer.quantity == Some(2) && offer.quantity_kind == QuantityKind::Listing)
    );
    assert!(first.offers.iter().any(|offer| offer.section.as_deref() == Some("C9") && offer.price_minor == Some(57500)));
}

#[test]
fn primary_ranges_and_selected_zero_are_categories_not_stock() {
    let primary = PAGE.split("<ul>\n  <li role=\"button\">").next().unwrap();
    let result = parser::parse(&watch(), primary, 10);
    assert_eq!(result.availability, Availability::Unknown);
    assert!(result.offers.is_empty());
    assert_eq!(result.ticket_categories.len(), 4);
    let seated = result
        .ticket_categories
        .iter()
        .find(|category| category.ticket_type == "Seated")
        .unwrap();
    assert_eq!(
        (seated.price_min_minor, seated.price_max_minor),
        (Some(17900), Some(37900))
    );
    assert_eq!(seated.selectable_quantity, Some(6));
    assert_eq!(seated.currency.as_deref(), Some("PLN"));
}

#[test]
fn localized_listing_labels_and_currency_can_change_without_new_selectors() {
    let english = PAGE
        .replace("Bilety Resale", "Resale Tickets")
        .replace("Dostępne ", "Available ")
        .replace("Sektor", "Section")
        .replace("Rząd", "Row")
        .replace("zł", "€");
    let result = parser::parse(&watch(), &english, 10);
    assert_eq!(result.offers.len(), 2);
    assert!(
        result
            .offers
            .iter()
            .all(|offer| offer.currency.as_deref() == Some("EUR") && offer.quantity == Some(2))
    );
}

#[test]
fn hidden_disabled_and_zero_listing_rows_do_not_report_available() {
    for modified in [
        PAGE.replace("<li role=\"button\">", "<li role=\"button\" hidden>"),
        PAGE.replace(
            "<li role=\"button\">",
            "<li role=\"button\" aria-disabled=\"true\">",
        ),
        PAGE.replace("Dostępne 2", "Dostępne 0"),
    ] {
        let result = parser::parse(&watch(), &modified, 10);
        assert_eq!(result.availability, Availability::Unknown);
        assert!(result.offers.is_empty());
    }
}

#[test]
fn purchase_limit_label_is_not_listing_quantity() {
    for label in [
        "Limit 6",
        "Dostępne 2–6",
        "Available up to 6",
        "Available 2 tickets, maximum 6",
    ] {
        let result = parser::parse(&watch(), &PAGE.replace("Dostępne 2", label), 10);
        assert!(result.offers.is_empty());
    }
}

#[test]
fn ambiguous_currency_does_not_become_a_price_in_an_assumed_market() {
    let dollars = PAGE.replace("zł", "$");
    let result = parser::parse(&watch(), &dollars, 10);
    assert_eq!(result.offers.len(), 2);
    assert!(
        result
            .offers
            .iter()
            .all(|offer| offer.currency.is_none() && offer.price_minor.is_none())
    );
    let mut configured = watch();
    configured.currency = Some("CAD".into());
    let resolved = parser::parse(&configured, &dollars, 10);
    assert!(
        resolved
            .offers
            .iter()
            .all(|offer| offer.currency.as_deref() == Some("CAD") && offer.price_minor.is_some())
    );
}

#[test]
fn conflicting_currency_preserves_unknown_amount() {
    let mut configured = watch();
    configured.currency = Some("EUR".into());
    let result = parser::parse(&configured, PAGE, 10);
    assert!(
        result
            .offers
            .iter()
            .all(|offer| offer.price_minor.is_none() && offer.currency.is_none())
    );
}

#[test]
fn changed_structures_fail_closed_instead_of_treating_any_number_as_inventory() {
    let result = parser::parse(
        &watch(),
        &PAGE.replace(
            "<span>Dostępne 2</span>",
            "<span>Sale ends in 2 days</span>",
        ),
        10,
    );
    assert!(result.offers.is_empty());
    assert_eq!(result.availability, Availability::Unknown);
    let changed = PAGE
        .replace("<span>575,00 zł</span>", "<strong>575,00 zł</strong>")
        .replace("<span>910,80 zł</span>", "<strong>910,80 zł</strong>");
    assert!(parser::parse(&watch(), &changed, 10).offers.is_empty());
}

#[test]
fn blocked_and_unknown_pages_do_not_use_metadata_as_fallback() {
    let blocked = parser::parse(
        &watch(),
        "<h1>Your Browsing Activity Has Been Paused</h1>",
        10,
    );
    assert_eq!(blocked.availability, Availability::Blocked);
    let unknown = parser::parse(&watch(), "<h1>Tickets available from 50 EUR</h1>", 10);
    assert_eq!(unknown.availability, Availability::Unknown);
    assert_eq!(unknown.adapter.as_deref(), Some("ticketmaster-unsupported"));
    let global = parser::parse(
        &watch(),
        "<ul id='list-view'><li role='menuitem' data-listing-id='offer'><button>Locked offer, $50</button></li></ul>",
        10,
    );
    assert_eq!(global.availability, Availability::Unknown);
    assert_eq!(
        global.adapter.as_deref(),
        Some("ticketmaster-global-unsupported")
    );
}

#[test]
fn generated_class_names_do_not_affect_extraction() {
    let altered = PAGE
        .replace("<span>", "<span class='a-new-build-hash'>")
        .replace("<div>", "<div class='different-generated-class'>");
    assert_eq!(
        parser::parse(&watch(), PAGE, 10).offers,
        parser::parse(&watch(), &altered, 10).offers
    );
}

#[test]
fn conflicting_duplicate_identity_cannot_emit_a_false_restock() {
    let one = PAGE
        .split("<li role=\"button\">")
        .nth(1)
        .unwrap()
        .split("</li>")
        .next()
        .unwrap();
    let duplicate = format!(
        "{PAGE}<li role='button'>{}</li>",
        one.replace("575,00", "580,00")
    );
    let result = parser::parse(&watch(), &duplicate, 10);
    assert_eq!(result.availability, Availability::Unknown);
    assert!(result.detail.contains("ambiguous duplicate"));
}

#[test]
fn unknown_price_range_is_not_converted_to_a_single_listing_price() {
    let result = parser::parse(
        &watch(),
        &PAGE.replace("575,00 zł", "575,00 zł–600,00 zł"),
        10,
    );
    let offer = result
        .offers
        .iter()
        .find(|offer| offer.section.as_deref() == Some("C9"))
        .unwrap();
    assert_eq!(offer.price_minor, None);
    assert_eq!(offer.quantity, Some(2));
}

#[test]
fn all_browser_challenge_markers_override_otherwise_readable_offers() {
    for marker in [
        "Pardon the interruption",
        "Request blocked",
        "Verify you're human",
        "Complete the CAPTCHA",
        "Solve the CAPTCHA",
        "Complete the reCAPTCHA",
        "You're in the queue",
        "Your place in the queue",
        "Waiting room",
        "Checking your browser",
        "Enable cookies to continue",
    ] {
        let result = parser::parse(&watch(), &format!("<h1>{marker}</h1>{PAGE}"), 10);
        assert_eq!(result.availability, Availability::Blocked, "{marker}");
        assert!(result.offers.is_empty());
        assert!(result.ticket_categories.is_empty());
        // Non-visible helper text and script strings are not a visible challenge.
        let hidden = format!("<script>{marker}</script><div hidden>{marker}</div>{PAGE}");
        assert_eq!(
            parser::parse(&watch(), &hidden, 10).availability,
            Availability::Available,
            "hidden {marker}"
        );
    }
}

#[test]
fn configured_currency_cannot_override_an_incompatible_symbol() {
    for (symbol, code) in [("$", "EUR"), ("kr", "PLN"), ("¥", "CAD")] {
        let mut configured = watch();
        configured.currency = Some(code.into());
        let result = parser::parse(&configured, &PAGE.replace("zł", symbol), 10);
        assert!(
            result
                .offers
                .iter()
                .all(|offer| offer.currency.is_none() && offer.price_minor.is_none()),
            "{symbol}/{code}"
        );
        assert!(
            result
                .ticket_categories
                .iter()
                .all(|category| category.currency.is_none()
                    && category.price_min_minor.is_none()
                    && category.price_max_minor.is_none())
        );
    }
    for (symbol, code) in [
        ("$", "USD"),
        ("$", "CAD"),
        ("kr", "SEK"),
        ("kr", "NOK"),
        ("¥", "JPY"),
        ("¥", "CNY"),
    ] {
        let mut configured = watch();
        configured.currency = Some(code.into());
        let result = parser::parse(&configured, &PAGE.replace("zł", symbol), 10);
        assert!(
            result
                .offers
                .iter()
                .all(|offer| offer.currency.as_deref() == Some(code)),
            "{symbol}/{code}"
        );
    }
}

#[test]
fn explicit_currency_and_mixed_symbols_must_agree() {
    for label in ["$ EUR", "kr PLN", "¥ CHF", "$ €", "USD £", "¥ kr", "€ £"] {
        let result = parser::parse(&watch(), &PAGE.replace("zł", label), 10);
        assert!(
            result
                .offers
                .iter()
                .all(|offer| offer.currency.is_none() && offer.price_minor.is_none()),
            "{label}"
        );
    }
}

#[test]
fn captured_czech_categories_and_resale_have_distinct_quantity_meanings() {
    let result = parser::parse(&watch(), CZECH, 10);
    assert_eq!(result.availability, Availability::Available);
    assert!(!result.complete);
    assert_eq!(result.ticket_categories.len(), 1);
    let category = &result.ticket_categories[0];
    assert_eq!(category.ticket_type, "Základní");
    assert_eq!(category.inventory_kind, InventoryKind::Unknown);
    assert_eq!(category.currency.as_deref(), Some("CZK"));
    assert_eq!(
        (category.price_min_minor, category.price_max_minor),
        (Some(149000), Some(1299000))
    );
    assert_eq!(category.selectable_quantity, Some(12));
    assert_eq!(result.offers.len(), 1);
    let offer = &result.offers[0];
    assert_eq!(offer.inventory_kind, InventoryKind::Resale);
    assert_eq!(offer.section.as_deref(), Some("413"));
    assert_eq!(offer.row.as_deref(), Some("2"));
    assert_eq!(offer.price_minor, Some(333500));
    assert_eq!(offer.currency.as_deref(), Some("CZK"));
    assert_eq!(offer.quantity, Some(2));
    assert_eq!(offer.quantity_kind, QuantityKind::Listing);
    assert!(offer.price_text.as_deref().unwrap().ends_with(" za kus"));
}

#[test]
fn captured_dutch_duplicate_cards_are_not_added_to_stock() {
    let result = parser::parse(&watch(), DUTCH, 10);
    assert_eq!(result.availability, Availability::Available);
    assert_eq!(result.ticket_categories.len(), 1);
    let category = &result.ticket_categories[0];
    assert_eq!(category.inventory_kind, InventoryKind::Unknown);
    assert_eq!(
        (category.price_min_minor, category.price_max_minor),
        (Some(4034), Some(5550))
    );
    assert_eq!(category.selectable_quantity, Some(4));
    assert_eq!(result.offers.len(), 1);
    let offer = &result.offers[0];
    assert_eq!(offer.inventory_kind, InventoryKind::Resale);
    assert_eq!(offer.ticket_type.as_deref(), Some("Reguliere verkoop"));
    assert_eq!(offer.price_minor, Some(7460));
    assert_eq!(offer.currency.as_deref(), Some("EUR"));
    assert_eq!(offer.quantity, Some(2));
    assert_eq!(offer.section.as_deref(), Some("Vak H"));
    assert_eq!(offer.row.as_deref(), Some("6"));
}

#[test]
fn localized_quantity_labels_still_reject_ranges_and_limits() {
    for label in [
        "Dostupných: 2–6",
        "Dostupných: až 6",
        "Dostupných: 0",
        "2–6 beschikbaar",
        "maximum 6 beschikbaar",
        "0 beschikbaar",
    ] {
        let result = parser::parse(&watch(), &CZECH.replace("Dostupných: 2 ", label), 10);
        assert!(result.offers.is_empty(), "{label}");
        assert_eq!(result.availability, Availability::Unknown, "{label}");
    }
}

fn primary(price: &str) -> String {
    format!(
        "<ul><li><div data-testid='ticketTypeInfo'><div><div><span>Standard</span><span>{price}</span></div></div></div><div data-testid='quantityStepper'><span role='spinbutton' aria-valuenow='0' aria-valuemax='6'>0</span></div></li></ul>"
    )
}

#[test]
fn captured_german_and_spanish_price_units_preserve_strict_money_semantics() {
    for (price, low, high) in [
        ("je 27,15 €", 2715, 2715),
        ("45,30 €–79,30 € cada una", 4530, 7930),
        ("147,30 € cada una", 14730, 14730),
        ("73,70 €–113,40 € cada una", 7370, 11340),
        ("39,70 €–79,30 € cada una", 3970, 7930),
    ] {
        let result = parser::parse(&watch(), &primary(price), 10);
        assert_eq!(result.availability, Availability::Unknown);
        assert!(result.offers.is_empty());
        let category = &result.ticket_categories[0];
        assert_eq!(
            (category.price_min_minor, category.price_max_minor),
            (Some(low), Some(high)),
            "{price}"
        );
        assert_eq!(category.currency.as_deref(), Some("EUR"));
        assert_eq!(category.selectable_quantity, Some(6));
    }
    for price in [
        "from 27,15 €",
        "je 27,15 € plus 5 €",
        "27,15 € cada una + fees",
        "27,15 € per stuk per stuk",
        "je 27,15 € cada una",
        "27,15 € za kus fee",
    ] {
        let result = parser::parse(&watch(), &primary(price), 10);
        let category = &result.ticket_categories[0];
        assert_eq!(
            (category.price_min_minor, category.price_max_minor),
            (None, None),
            "{price}"
        );
    }
}

fn swedish_watch() -> Watch {
    let mut watch = watch();
    watch.url = "https://www.ticketmaster.se/event/example-biljetter/123456789".into();
    watch
}

#[test]
fn matching_event_currency_resolves_swedish_kr_without_importing_metadata_inventory() {
    let result = parser::parse(&swedish_watch(), SWEDISH, 10);
    assert_eq!(result.availability, Availability::Unknown);
    assert!(result.offers.is_empty());
    assert_eq!(result.ticket_categories.len(), 2);
    assert!(!result.complete);
    for category in &result.ticket_categories {
        assert_eq!(category.currency.as_deref(), Some("SEK"));
        assert_eq!(
            (category.price_min_minor, category.price_max_minor),
            (Some(43500), Some(43500))
        );
        assert_eq!(category.selectable_quantity, Some(10));
    }
    let no_displayed_prices = SWEDISH.replace("435,00 kr per biljett", "Price pending");
    let result = parser::parse(&swedish_watch(), &no_displayed_prices, 10);
    assert_eq!(result.availability, Availability::Unknown);
    assert!(result.ticket_categories.is_empty());
    let mut query_watch = swedish_watch();
    query_watch.url =
        "https://ticketmaster.se/event/new-slug/123456789?language=en-us#tickets".into();
    assert_eq!(
        parser::parse(&query_watch, SWEDISH, 10).ticket_categories[0]
            .currency
            .as_deref(),
        Some("SEK")
    );
}

#[test]
fn currency_metadata_requires_matching_event_identity_and_compatible_symbols() {
    for page in [
        SWEDISH.replace("123456789", "987654321"),
        SWEDISH.replace("ticketmaster.se/event", "ticketmaster.no/event"),
        SWEDISH.replace("\"MusicEvent\"", "\"Organization\""),
        SWEDISH.replace("\"priceCurrency\":\"SEK\",", ""),
        SWEDISH.replace("\"SEK\"", "\"EUR\""),
        SWEDISH.replacen(
            "\"url\":\"https://www.ticketmaster.se/event/example-biljetter/123456789\"}",
            "\"url\":\"https://www.ticketmaster.se/event/wrong/987654321\"}",
            1,
        ),
    ] {
        let result = parser::parse(&swedish_watch(), &page, 10);
        assert!(result.ticket_categories.iter().all(|c| c.currency.is_none()
            && c.price_min_minor.is_none()
            && c.price_max_minor.is_none()));
    }
    // Country and language cannot determine which currency an ambiguous symbol uses.
    let no_metadata = SWEDISH.split("</head>").nth(1).unwrap();
    let result = parser::parse(&swedish_watch(), no_metadata, 10);
    assert!(
        result
            .ticket_categories
            .iter()
            .all(|c| c.currency.is_none())
    );
    let mut configured = swedish_watch();
    configured.currency = Some("SEK".into());
    let result = parser::parse(&configured, no_metadata, 10);
    assert!(
        result
            .ticket_categories
            .iter()
            .all(|c| c.currency.as_deref() == Some("SEK") && c.price_min_minor == Some(43500))
    );
}

fn metadata(currency: &str) -> String {
    format!(
        r#"<script type="application/ld+json">{{"@type":"Event","url":"https://www.ticketmaster.se/event/example/123456789","offers":{{"priceCurrency":"{currency}"}}}}</script>"#
    )
}

#[test]
fn conflicting_currency_hints_cannot_be_overridden_by_configuration_or_visible_prices() {
    for (page, configured) in [
        (format!("{SWEDISH}{}", metadata("NOK")), None),
        (
            format!("{SWEDISH}{}", metadata("NOT_A_CURRENCY")),
            Some("SEK"),
        ),
        (SWEDISH.into(), Some("NOK")),
        (SWEDISH.replace("435,00 kr", "435,00 EUR"), None),
    ] {
        let mut watch = swedish_watch();
        watch.currency = configured.map(str::to_string);
        let result = parser::parse(&watch, &page, 10);
        assert!(
            result
                .ticket_categories
                .iter()
                .all(|c| c.currency.is_none() && c.price_min_minor.is_none())
        );
    }
    let other_event = metadata("NOK").replace("123456789", "987654321");
    let result = parser::parse(&swedish_watch(), &format!("{SWEDISH}{other_event}"), 10);
    assert!(
        result
            .ticket_categories
            .iter()
            .all(|c| c.currency.as_deref() == Some("SEK"))
    );
}

#[test]
fn currency_metadata_bounds_do_not_accept_partially_scanned_evidence() {
    let nested = format!("{}{}{}", "[".repeat(26), "{}", "]".repeat(26));
    let wide = serde_json::to_string(&vec!["unrelated"; 4097]).unwrap();
    let oversized = serde_json::to_string(&"x".repeat(256 * 1024)).unwrap();
    for extra in [nested, wide, oversized, "{malformed".into()] {
        let page = format!("{SWEDISH}<script type='application/ld+json'>{extra}</script>");
        let result = parser::parse(&swedish_watch(), &page, 10);
        assert!(
            result
                .ticket_categories
                .iter()
                .all(|c| c.currency.is_none())
        );
    }
    let page = format!(
        "{SWEDISH}{}",
        "<script type='application/ld+json'>{}</script>".repeat(16)
    );
    let result = parser::parse(&swedish_watch(), &page, 10);
    assert!(
        result
            .ticket_categories
            .iter()
            .all(|c| c.currency.is_none())
    );
}

#[test]
fn currency_hint_supports_matched_graph_events_and_nested_aggregate_offers() {
    let page = format!(
        r#"<script type="application/ld+json">{{"@graph":[{{"@type":["Thing","https://schema.org/MusicEvent"],"@id":"https://ticketmaster.se/event/another-slug/123456789?language=en-us","offers":{{"@type":"AggregateOffer","offers":[{{"@type":"Offer","priceCurrency":"SEK"}},{{"@type":"Offer","priceCurrency":"SEK","url":"/event/example/123456789"}}]}}}}]}}</script>{}"#,
        primary("435,00 kr per biljett")
    );
    let result = parser::parse(&swedish_watch(), &page, 10);
    assert_eq!(result.ticket_categories[0].currency.as_deref(), Some("SEK"));
    assert_eq!(result.ticket_categories[0].price_min_minor, Some(43500));
    assert_eq!(result.availability, Availability::Unknown);
    let conflict = page.replacen("\"priceCurrency\":\"SEK\"", "\"priceCurrency\":\"NOK\"", 1);
    let result = parser::parse(&swedish_watch(), &conflict, 10);
    assert_eq!(result.ticket_categories[0].currency, None);
}

#[test]
fn captured_british_quick_pick_prices_do_not_turn_search_filters_into_stock() {
    let result = parser::parse(&watch(), BRITISH, 10);
    assert_eq!(result.availability, Availability::Unknown);
    assert!(result.offers.is_empty());
    assert_eq!(result.ticket_categories.len(), 1);
    let category = &result.ticket_categories[0];
    assert_eq!(category.ticket_type, "Full Price Ticket");
    assert_eq!(category.currency.as_deref(), Some("GBP"));
    assert_eq!(
        (category.price_min_minor, category.price_max_minor),
        (Some(4035), Some(4035))
    );
    assert_eq!(category.selectable_quantity, None);
    assert_eq!(category.section.as_deref(), Some("General Admission"));
    assert_eq!(category.row, None);
    let changed_filter = BRITISH.replace("2 Tickets", "6 Tickets");
    assert_eq!(
        result.ticket_categories,
        parser::parse(&watch(), &changed_filter, 10).ticket_categories
    );
    for changed in [
        BRITISH.replace("data-testid=\"quickpicksList\"", "data-testid=\"unknown\""),
        BRITISH.replace("<dt>Section</dt>", "<dt>Unknown field</dt>"),
        BRITISH.replace(
            "role=\"button\" tabindex",
            "role=\"button\" aria-disabled=\"true\" tabindex",
        ),
        BRITISH.replace(
            "role=\"button\" tabindex",
            "role=\"button\" hidden tabindex",
        ),
    ] {
        let result = parser::parse(&watch(), &changed, 10);
        assert!(result.ticket_categories.is_empty());
        assert!(result.offers.is_empty());
    }
}

#[test]
fn quick_pick_category_identity_preserves_verified_section_and_row_context() {
    let result = parser::parse(&watch(), BRITISH_SEATED, 10);
    assert_eq!(result.availability, Availability::Unknown);
    assert!(result.offers.is_empty());
    assert!(!result.complete);
    assert_eq!(result.ticket_categories.len(), 2);
    for (section, row, price) in [("507", "11", 15575), ("315", "5", 19900)] {
        let category = result
            .ticket_categories
            .iter()
            .find(|c| c.section.as_deref() == Some(section))
            .unwrap();
        assert_eq!(category.row.as_deref(), Some(row));
        assert_eq!(category.ticket_type, "Side View Restricted Seated Ticket");
        assert_eq!(category.price_min_minor, Some(price));
        assert_eq!(category.price_max_minor, Some(price));
        assert_eq!(category.currency.as_deref(), Some("GBP"));
        assert_eq!(category.selectable_quantity, None);
    }
    // A different row in the same section also has distinct visible context.
    let same_section = BRITISH_SEATED.replace("<dd>315</dd>", "<dd>507</dd>");
    assert_eq!(
        parser::parse(&watch(), &same_section, 10)
            .ticket_categories
            .len(),
        2
    );
    let duplicate = format!("{BRITISH_SEATED}{BRITISH_SEATED}");
    assert_eq!(
        parser::parse(&watch(), &duplicate, 10).ticket_categories,
        result.ticket_categories
    );
}

#[test]
fn conflicting_prices_in_the_same_quick_pick_context_remain_unknown() {
    let same_context = BRITISH_SEATED
        .replace("<dd>315</dd>", "<dd>507</dd>")
        .replace("<dd>5</dd>", "<dd>11</dd>");
    let result = parser::parse(&watch(), &same_context, 10);
    assert_eq!(result.availability, Availability::Unknown);
    assert!(result.ticket_categories.is_empty());
    assert!(result.offers.is_empty());
    assert!(
        result
            .detail
            .contains("ambiguous duplicate Ticketmaster category")
    );
    let primary_conflict = format!("{}{}", primary("£155.75 each"), primary("£199.00 each"));
    let result = parser::parse(&watch(), &primary_conflict, 10);
    assert!(result.ticket_categories.is_empty());
    assert!(
        result
            .detail
            .contains("ambiguous duplicate Ticketmaster category")
    );
    let category = &parser::parse(&watch(), &primary("£155.75 each"), 10).ticket_categories[0];
    assert_eq!(category.section, None);
    assert_eq!(category.row, None);
    let mut old_category = serde_json::to_value(category).unwrap();
    old_category.as_object_mut().unwrap().remove("section");
    old_category.as_object_mut().unwrap().remove("row");
    assert_eq!(
        *category,
        serde_json::from_value::<ticket_watch::model::TicketCategory>(old_category).unwrap()
    );
}
