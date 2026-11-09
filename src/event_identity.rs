//! Match a public event after redirects before accepting any inventory evidence.
use crate::config::validate_public_url;

/// An artist page, home page, or a different same-host event cannot supply offers.
/// Unknown URL shapes fail closed; only an ordinary event ID is compared.
pub fn same_event(expected: &str, actual: &str) -> bool {
    let Some((expected_host, expected_id)) = identity(expected) else {
        return false;
    };
    let Some((actual_host, actual_id)) = identity(actual) else {
        return false;
    };
    expected_host == actual_host && expected_id == actual_id
}

pub(crate) fn identity(input: &str) -> Option<(String, String)> {
    // url::Url normalizes dot segments and backslashes. Reject ambiguous raw paths
    // first so normalization cannot turn a different route into an apparent event.
    if input.len() > 4096
        || input.trim() != input
        || input.chars().any(char::is_control)
        || input.contains('\\')
    {
        return None;
    }
    let before_query = input.split(['?', '#']).next()?;
    let (_, authority_path) = before_query.split_once("://")?;
    let raw_path = authority_path
        .find('/')
        .map(|index| &authority_path[index..])
        .unwrap_or("");
    if raw_path.contains("//") {
        return None;
    }
    let url = validate_public_url(input).ok()?;
    let host = url.host_str()?;
    let host = host.strip_prefix("www.").unwrap_or(host).to_owned();
    // Decode individual raw segments only for slug validation. Structural route
    // markers and event IDs remain literal, so escaped separators/IDs cannot
    // silently change which route is being compared.
    let path = raw_path.strip_suffix('/').unwrap_or(raw_path);
    let parts: Vec<_> = path.strip_prefix('/')?.split('/').collect();
    let decoded = parts
        .iter()
        .map(|part| decode_segment(part))
        .collect::<Option<Vec<_>>>()?;
    let id = match parts.as_slice() {
        ["event", id] => *id,
        ["event", _, id] if safe_slug(&decoded[1]) => *id,
        [_, "event", id] if safe_slug(&decoded[0]) => *id,
        [locale, "event", _, id] if is_locale(locale) && safe_slug(&decoded[2]) => *id,
        [locale, _, "event", id] if is_locale(locale) && safe_slug(&decoded[1]) => *id,
        ["biglietti", _, "event", id] if host == "ticketmaster.it" && safe_slug(&decoded[1]) => *id,
        [locale, "biglietti", _, "event", id]
            if host == "ticketmaster.it" && is_locale(locale) && safe_slug(&decoded[2]) =>
        {
            *id
        }
        _ => return None,
    };
    let numeric = !id.is_empty() && id.len() <= 20 && id.bytes().all(|byte| byte.is_ascii_digit());
    let alphanumeric = (8..=64).contains(&id.len())
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
        && id.bytes().any(|byte| byte.is_ascii_alphabetic())
        && id.bytes().any(|byte| byte.is_ascii_digit());
    if !numeric && !alphanumeric {
        return None;
    }
    Some((host, id.to_ascii_uppercase()))
}

fn decode_segment(value: &str) -> Option<String> {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = char::from(*bytes.get(index + 1)?).to_digit(16)?;
            let low = char::from(*bytes.get(index + 2)?).to_digit(16)?;
            decoded.push((high * 16 + low) as u8);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).ok()?;
    if decoded.is_empty()
        || matches!(decoded.as_str(), "." | "..")
        || decoded.chars().any(char::is_control)
        || decoded.contains(['/', '\\', '%', '?', '#'])
    {
        return None;
    }
    Some(decoded)
}

fn is_locale(value: &str) -> bool {
    let parts: Vec<_> = value.split('-').collect();
    (1..=2).contains(&parts.len())
        && parts
            .iter()
            .all(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_alphabetic()))
}

fn safe_slug(value: &str) -> bool {
    !value.is_empty()
        && ![
            "event",
            "artist",
            "account",
            "login",
            "payment",
            "checkout",
            "cart",
            "help",
            "recommendation",
            "recommendations",
            "search",
        ]
        .contains(&value.to_ascii_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KORN: &str = "https://www.ticketmaster.pl/event/korn-bilety/496543017";

    #[test]
    fn accepts_regional_slug_changes_and_public_host_alias() {
        assert!(same_event(
            KORN,
            "https://ticketmaster.pl/event/renamed-show/496543017/?language=en#tickets"
        ));
        assert!(same_event(
            KORN,
            "https://www.ticketmaster.pl/event/496543017"
        ));
        assert!(same_event(
            "https://www.ticketmaster.com/korn-tickets/event/0600634BCF8171E6",
            "https://www.ticketmaster.com/event/0600634bcf8171e6"
        ));
    }

    #[test]
    fn rejects_different_event_or_unknown_route() {
        for actual in [
            "https://www.ticketmaster.pl/event/korn-bilety/999999999",
            "https://www.ticketmaster.cz/event/korn-bilety/496543017",
            "https://secure.ticketmaster.pl/event/korn-bilety/496543017",
            "https://www.ticketmaster.pl/",
            "https://www.ticketmaster.pl/korn/artist/496543017",
            "https://www.ticketmaster.pl/login/event/496543017",
            "https://www.ticketmaster.pl/recommendations/event/496543017",
            "https://www.ticketmaster.pl/event/korn-bilety/496543017/details",
            "https://www.ticketmaster.pl/event/event/496543017",
            "https://www.ticketmaster.pl/event/korn",
        ] {
            assert!(!same_event(KORN, actual), "Accepted {actual}");
        }
    }

    #[test]
    fn rejects_ambiguous_encoding_credentials_and_path_normalization() {
        for actual in [
            "https://www.ticketmaster.pl/event/korn/%34496543017",
            "https://www.ticketmaster.pl/login/../event/496543017",
            "https://www.ticketmaster.pl/login/%2e%2e/event/496543017",
            "https://www.ticketmaster.pl/event//496543017",
            "https://www.ticketmaster.pl/event/korn%2fother/496543017",
            "https://www.ticketmaster.pl/event\\korn/496543017",
            "https://www.ticketmaster.pl/event/496543017?token=secret",
            "https://user:secret@www.ticketmaster.pl/event/496543017",
            "http://www.ticketmaster.pl/event/496543017",
            " https://www.ticketmaster.pl/event/496543017",
        ] {
            assert!(!same_event(KORN, actual), "Accepted {actual}");
        }
    }

    #[test]
    fn an_unrecognized_expected_url_cannot_match_itself() {
        for url in [
            "https://www.ticketmaster.pl/korn/artist/496543017",
            "https://www.ticketmaster.pl/event/korn",
            "https://www.ticketmaster.pl/event/123456789012345678901",
        ] {
            assert!(!same_event(url, url));
        }
    }

    #[test]
    fn accepts_encoded_slugs_locales_and_italian_event_routes() {
        for actual in [
            "https://www.ticketmaster.de/event/show-%7C-logen-seat/12345",
            "https://www.ticketmaster.de/event/bj%C3%B6rk/12345",
            "https://www.ticketmaster.de/en/event/new-title/12345",
            "https://www.ticketmaster.de/en-de/new-title/event/12345",
            "https://www.ticketmaster.de/event/björk/12345",
        ] {
            assert!(
                same_event("https://www.ticketmaster.de/event/12345", actual),
                "Rejected {actual}"
            );
        }
        for actual in [
            "https://www.ticketmaster.it/biglietti/another-artist-roma/event/qfd2zy7m15y4",
            "https://www.ticketmaster.it/it/biglietti/another-artist-roma/event/qfd2zy7m15y4",
        ] {
            assert!(
                same_event("https://www.ticketmaster.it/event/qfd2zy7m15y4", actual),
                "Rejected {actual}"
            );
        }
    }

    #[test]
    fn rejects_escaped_routes_and_ambiguous_locale_paths() {
        for path in [
            "/event/hello%2fworld/12345",
            "/event/hello%5cworld/12345",
            "/event/hello%252fworld/12345",
            "/event/%2e%2e/12345",
            "/event/%63heckout/12345",
            "/%65vent/show/12345",
            "/event/show/%31%32%33%34%35",
            "/event/show%3fcheckout/12345",
            "/event/show%00/12345",
            "/event/show%80/12345",
            "/event/show%/12345",
            "/english/event/title/12345",
            "/en/event/checkout/12345",
            "/biglietti/title/event/12345",
            "/event/show/12345/extra",
        ] {
            let actual = format!("https://www.ticketmaster.de{path}");
            assert!(
                !same_event("https://www.ticketmaster.de/event/12345", &actual),
                "Accepted {actual}"
            );
        }
    }
}
