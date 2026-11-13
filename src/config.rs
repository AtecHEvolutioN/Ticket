use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub database: PathBuf,
    pub map_database: PathBuf,
    pub request_timeout_secs: u64,
    pub per_host_interval_secs: u64,
    pub blocked_retry_secs: u64,
    pub unknown_retry_secs: u64,
    pub max_in_flight: usize,
    pub max_body_bytes: usize,
    pub alert_max_age_secs: u64,
    pub browser: BrowserConfig,
    pub discord: DiscordConfig,
    pub watches: Vec<Watch>,
    pub map_watches: Vec<MapWatch>,
    pub catalog: CatalogConfig,
    pub subscriptions: Vec<Subscription>,
    pub profiles: Vec<WebsiteProfile>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database: "data/ticket.sqlite".into(),
            map_database: "data/map-monitor.sqlite".into(),
            request_timeout_secs: 20,
            per_host_interval_secs: 60,
            blocked_retry_secs: 900,
            unknown_retry_secs: 120,
            max_in_flight: 4,
            max_body_bytes: 4 * 1024 * 1024,
            alert_max_age_secs: 300,
            browser: BrowserConfig::default(),
            discord: DiscordConfig::default(),
            watches: vec![],
            map_watches: vec![],
            catalog: CatalogConfig::default(),
            subscriptions: vec![],
            profiles: vec![],
        }
    }
}

/// One selected section's displayed-price checks; never a purchase instruction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct MapWatch {
    pub id: String,
    pub name: String,
    pub url: String,
    pub section_id: String,
    pub limit: u32,
    pub interval_secs: u64,
    pub enabled: bool,
    pub notify_initial: bool,
    pub notify_price_changes: bool,
    pub notify_details_changes: bool,
}

impl Default for MapWatch {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            url: String::new(),
            section_id: String::new(),
            limit: 20,
            interval_secs: 60,
            enabled: true,
            notify_initial: false,
            notify_price_changes: true,
            notify_details_changes: false,
        }
    }
}

/// Discovery refresh budgets are separate from website request pacing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CatalogConfig {
    pub refresh_interval_secs: u64,
    pub max_requests_per_refresh: usize,
    pub max_requests_per_day: u32,
    pub max_events: usize,
    /// Split all-artist country searches into resumable bounded UTC date ranges.
    pub country_partitioning: bool,
    pub country_horizon_days: u32,
    pub country_window_days: u32,
}
impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            refresh_interval_secs: 900,
            max_requests_per_refresh: 20,
            max_requests_per_day: 4000,
            max_events: 1000,
            country_partitioning: false,
            country_horizon_days: 730,
            country_window_days: 30,
        }
    }
}

/// Artist subscriptions require an explicit attraction ID. Broad discovery must
/// opt into all_artists and provide a country instead of silently widening scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Subscription {
    pub id: String,
    pub artist: String,
    pub attraction_id: String,
    #[serde(skip_serializing_if = "is_false")]
    pub all_artists: bool,
    pub country: Option<String>,
    pub city: Option<String>,
    pub enabled: bool,
    pub profile: String,
    pub exclude_keywords: Vec<String>,
}
impl Default for Subscription {
    fn default() -> Self {
        Self {
            id: String::new(),
            artist: String::new(),
            attraction_id: String::new(),
            all_artists: false,
            country: None,
            city: None,
            enabled: true,
            profile: String::new(),
            exclude_keywords: vec![],
        }
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

/// A website extraction profile is valid only for explicitly listed public hosts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebsiteProfile {
    pub id: String,
    pub hosts: Vec<String>,
    pub mode: FetchMode,
    pub interval_secs: u64,
    pub notify_initial: bool,
    pub notify_offer_changes: bool,
    pub min_quantity: Option<u32>,
    pub max_price_minor: Option<i64>,
    pub currency: Option<String>,
    pub parser: ParserConfig,
}
impl Default for WebsiteProfile {
    fn default() -> Self {
        Self {
            id: String::new(),
            hosts: vec![],
            mode: FetchMode::Http,
            interval_secs: 60,
            notify_initial: false,
            notify_offer_changes: false,
            min_quantity: None,
            max_price_minor: None,
            currency: None,
            parser: ParserConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserConfig {
    pub node: String,
    pub script: PathBuf,
    pub executable_path: Option<String>,
    pub render_wait_ms: u64,
    pub headed: bool,
}
impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            node: "node".into(),
            script: "browser/worker.cjs".into(),
            executable_path: None,
            render_wait_ms: 1500,
            headed: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiscordConfig {
    pub channel_id: Option<String>,
    pub token_env: String,
    pub mention_user_ids: Vec<String>,
    pub mention_role_ids: Vec<String>,
}
impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            channel_id: None,
            token_env: "DISCORD_BOT_TOKEN".into(),
            mention_user_ids: vec![],
            mention_role_ids: vec![],
        }
    }
}

impl DiscordConfig {
    pub fn validate(&self) -> Result<()> {
        if self
            .channel_id
            .as_deref()
            .is_some_and(|id| !discord_snowflake(id))
            || self
                .mention_user_ids
                .iter()
                .chain(&self.mention_role_ids)
                .any(|id| !discord_snowflake(id))
        {
            bail!(
                "Discord channel/user/role IDs must be positive numeric snowflakes fitting in 64 bits"
            );
        }
        if self.mention_user_ids.len() + self.mention_role_ids.len() > 50 {
            bail!("Configure at most 50 Discord mention targets");
        }
        if self.token_env.is_empty()
            || !self
                .token_env
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            bail!("Discord token_env must be an environment variable name, never the token itself");
        }
        Ok(())
    }
}

pub(crate) fn discord_snowflake(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 20
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|n| n > 0)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FetchMode {
    #[default]
    Http,
    Browser,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Watch {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub country: String,
    pub city: Option<String>,
    pub url: String,
    pub enabled: bool,
    pub mode: FetchMode,
    pub interval_secs: u64,
    pub notify_initial: bool,
    pub notify_metadata: bool,
    pub notify_offer_changes: bool,
    pub min_quantity: Option<u32>,
    pub max_price_minor: Option<i64>,
    pub currency: Option<String>,
    pub parser: ParserConfig,
}
impl Default for Watch {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            artist: String::new(),
            country: String::new(),
            city: None,
            url: String::new(),
            enabled: true,
            mode: FetchMode::Http,
            interval_secs: 60,
            notify_initial: false,
            notify_metadata: false,
            notify_offer_changes: false,
            min_quantity: None,
            max_price_minor: None,
            currency: None,
            parser: ParserConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PageAdapter {
    #[default]
    Selectors,
    Ticketmaster,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ParserConfig {
    pub adapter: PageAdapter,
    pub selectors_verified: bool,
    pub available_selector: Option<String>,
    pub unavailable_selector: Option<String>,
    pub offers_selector: Option<String>,
    /// A visible, enabled purchasable signal within each matched offer row.
    pub offer_available_selector: Option<String>,
    pub complete_selector: Option<String>,
    pub offer_id_attribute: Option<String>,
    pub section_selector: Option<String>,
    pub row_selector: Option<String>,
    pub seats_selector: Option<String>,
    pub ticket_type_selector: Option<String>,
    pub inventory_selector: Option<String>,
    pub price_selector: Option<String>,
    pub quantity_selector: Option<String>,
    pub quantity_is_stock: bool,
    /// Explicit semantics take priority; legacy quantity_is_stock maps Unknown to Remaining.
    pub quantity_kind: crate::model::QuantityKind,
    pub price_scope: crate::model::PriceScope,
    pub fees: crate::model::FeeScope,
    #[serde(default = "default_decimal_separator")]
    pub price_decimal_separator: String,
}
fn default_decimal_separator() -> String {
    ".".into()
}

pub fn validate_public_url(input: &str) -> Result<Url> {
    let url = Url::parse(input).context("Invalid public event URL")?;
    let host = url.host_str().unwrap_or("");
    let allowed = [
        "ticketmaster.pl",
        "ticketmaster.cz",
        "ticketmaster.com",
        "ticketmaster.co.uk",
        "ticketmaster.de",
        "ticketmaster.at",
        "ticketmaster.ch",
        "ticketmaster.be",
        "ticketmaster.nl",
        "ticketmaster.ie",
        "ticketmaster.es",
        "ticketmaster.it",
        "ticketmaster.fr",
        "ticketmaster.dk",
        "ticketmaster.no",
        "ticketmaster.se",
        "ticketmaster.fi",
        "ticketmaster.ca",
        "ticketmaster.com.au",
        "ticketmaster.co.nz",
        "ticketmaster.com.mx",
        "ticketmaster.ae",
        "ticketmaster.co.za",
    ];
    let normal_host = host.strip_prefix("www.").unwrap_or(host);
    if url.scheme() != "https"
        || !allowed.contains(&normal_host)
        || url.port().is_some_and(|v| v != 443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!(
            "Use a public HTTPS Ticketmaster event/artist URL, without credentials or a custom port"
        );
    }
    let p = url.path();
    if !(p.contains("/event/") || p.contains("/artist/")) {
        bail!(
            "Only public /event/ and /artist/ pages are supported; payment/cart links are not watch targets"
        );
    }
    for (key, _) in url.query_pairs() {
        if !["language", "locale", "page"].contains(&key.as_ref()) {
            bail!("Remove tracking or session parameters from the event URL");
        }
    }
    Ok(url)
}

pub fn load(path: &Path) -> Result<Config> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Read configuration {}", path.display()))?;
    let mut config: Config = toml::from_str(&raw).context("Invalid TOML configuration")?;
    let root = path.parent().unwrap_or(Path::new("."));
    if config.database.is_relative() {
        config.database = root.join(&config.database);
    }
    if config.map_database.is_relative() {
        config.map_database = root.join(&config.map_database);
    }
    if config.browser.script.is_relative() {
        config.browser.script = root.join(&config.browser.script);
    }
    validate(&config)?;
    Ok(config)
}

pub fn validate(config: &Config) -> Result<()> {
    if normalized_database_path(&config.database)?
        == normalized_database_path(&config.map_database)?
    {
        bail!("Map monitoring requires a separate map_database from the inventory database");
    }
    if config.request_timeout_secs == 0
        || config.request_timeout_secs > 120
        || config.per_host_interval_secs == 0
        || config.blocked_retry_secs < 60
        || config.unknown_retry_secs == 0
        || !(1..=32).contains(&config.max_in_flight)
        || !(1024..=16 * 1024 * 1024).contains(&config.max_body_bytes)
        || config.alert_max_age_secs == 0
        || config.browser.render_wait_ms > 30_000
        || config.per_host_interval_secs > 31_536_000
        || config.blocked_retry_secs > 31_536_000
        || config.unknown_retry_secs > 31_536_000
        || config.alert_max_age_secs > 31_536_000
    {
        bail!(
            "Invalid limits: use bounded timeouts, positive intervals, 1–32 workers, 1KB–16MB pages, and blocked cooldown >=60s"
        );
    }
    config.discord.validate()?;
    if config.map_watches.len() > 1000 {
        bail!("Configure at most 1000 map watches");
    }
    let mut map_ids = HashSet::new();
    let mut map_targets = HashSet::new();
    for watch in &config.map_watches {
        if !safe_id(&watch.id) || !map_ids.insert(watch.id.as_str()) {
            bail!(
                "Each map watch needs a unique bounded id using letters, digits, hyphens or underscores"
            );
        }
        if !bounded_label(&watch.name, 500)
            || watch.section_id.is_empty()
            || watch.section_id.len() > 128
            || !watch
                .section_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || !(1..=50).contains(&watch.limit)
            || !(1..=31_536_000).contains(&watch.interval_secs)
        {
            bail!(
                "Map watch {} has an invalid name, section id, scan limit or interval",
                watch.id
            );
        }
        let (host, event) = crate::event_identity::identity(&watch.url)
            .filter(|(host, _)| host == "ticketmaster.cz")
            .context("Map watches require a supported public Czech event URL")?;
        if watch.enabled && !map_targets.insert((host, event, watch.section_id.as_str())) {
            bail!("Duplicate active map target: use one watch per event section");
        }
    }
    let mut ids = HashSet::new();
    let mut urls = HashSet::new();
    for watch in &config.watches {
        if watch.id.starts_with("auto-")
            || watch.id.is_empty()
            || watch.id.len() > 100
            || !ids.insert(watch.id.clone())
            || !watch
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!(
                "Each manual watch needs a unique id using letters, digits, hyphens, or underscores; auto- is reserved"
            );
        }
        if watch.name.trim().is_empty()
            || watch.artist.trim().is_empty()
            || watch.country.len() != 2
            || !watch.country.bytes().all(|b| b.is_ascii_uppercase())
            || watch.interval_secs == 0
            || watch.interval_secs > 31_536_000
            || watch.name.len() > 500
            || watch.artist.len() > 500
        {
            bail!(
                "Watch {} needs a name, artist, uppercase country code, and positive interval",
                watch.id
            );
        }
        let mut url = validate_public_url(&watch.url)?;
        if !url.path().contains("/event/") {
            bail!(
                "Watch {} must target an event; use discover for artist pages",
                watch.id
            );
        }
        url.set_fragment(None);
        url.set_query(None);
        if watch.enabled && !urls.insert(url.to_string()) {
            bail!("Duplicate active event URL: use one watch per event to share checks");
        }
        if watch.max_price_minor.is_some_and(|v| v < 0)
            || watch.min_quantity == Some(0)
            || watch
                .currency
                .as_ref()
                .is_some_and(|s| s.len() != 3 || !s.bytes().all(|b| b.is_ascii_uppercase()))
            || (watch.max_price_minor.is_some() && watch.currency.is_none())
        {
            bail!(
                "Watch {} has invalid price/quantity filters; price ceilings require a currency",
                watch.id
            );
        }
        for selector in [
            &watch.parser.available_selector,
            &watch.parser.unavailable_selector,
            &watch.parser.offers_selector,
            &watch.parser.offer_available_selector,
            &watch.parser.complete_selector,
            &watch.parser.section_selector,
            &watch.parser.row_selector,
            &watch.parser.seats_selector,
            &watch.parser.ticket_type_selector,
            &watch.parser.inventory_selector,
            &watch.parser.price_selector,
            &watch.parser.quantity_selector,
        ]
        .into_iter()
        .flatten()
        {
            if watch.parser.adapter == PageAdapter::Ticketmaster {
                bail!(
                    "Watch {} cannot combine the built-in Ticketmaster adapter with manual selectors",
                    watch.id
                );
            }
            if selector.len() > 500 || scraper::Selector::parse(selector).is_err() {
                bail!("Watch {} has an invalid CSS selector", watch.id);
            }
        }
        if !["", ".", ","].contains(&watch.parser.price_decimal_separator.as_str()) {
            bail!("price_decimal_separator must be '.' or ','");
        }
        if watch.parser.selectors_verified
            && watch.parser.offers_selector.is_some()
            && watch.parser.offer_available_selector.is_none()
        {
            bail!(
                "Watch {} needs offer_available_selector before its offer selectors can be verified",
                watch.id
            );
        }
    }
    validate_catalog(config)?;
    Ok(())
}

fn normalized_database_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        bail!("Database paths must not be empty");
    }
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .context("Resolve database path")?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn bounded_label(label: &str, max: usize) -> bool {
    !label.trim().is_empty() && label.len() <= max && !label.chars().any(char::is_control)
}

fn validate_catalog(config: &Config) -> Result<()> {
    let catalog = &config.catalog;
    if !(60..=31_536_000).contains(&catalog.refresh_interval_secs)
        || !(1..=1000).contains(&catalog.max_requests_per_refresh)
        || !(1..=5000).contains(&catalog.max_requests_per_day)
        || catalog.max_requests_per_refresh as u32 > catalog.max_requests_per_day
        || !(1..=10_000).contains(&catalog.max_events)
        || !(1..=3650).contains(&catalog.country_horizon_days)
        || !(1..=365).contains(&catalog.country_window_days)
        || catalog.country_window_days > catalog.country_horizon_days
        || config.subscriptions.len() > 1000
        || config.profiles.len() > 100
    {
        bail!(
            "Invalid catalog limits: refresh >=60s, 1–1000 requests/refresh within daily budget, 1–10000 events, country horizon 1–3650 days, and window 1–365 days within the horizon"
        );
    }
    let mut profile_ids = HashSet::new();
    for profile in &config.profiles {
        if !safe_id(&profile.id) || !profile_ids.insert(profile.id.as_str()) {
            bail!("Website profile ids must be unique letters, digits, hyphens, or underscores");
        }
        if profile.hosts.is_empty() || profile.hosts.len() > 64 {
            bail!(
                "Profile {} needs 1–64 explicit public Ticketmaster hosts",
                profile.id
            );
        }
        let mut hosts = HashSet::new();
        for host in &profile.hosts {
            let url = validate_public_url(&format!("https://{host}/event/profile-validation"))?;
            if url.host_str() != Some(host.as_str()) || !hosts.insert(host) {
                bail!(
                    "Profile {} hosts must be unique lowercase host names without paths or ports",
                    profile.id
                );
            }
        }
        // Exercise exactly the same parsing, price, quantity, and interval validation
        // as a manual watch, without duplicating those rules here.
        let synthetic = Watch {
            id: "profile-validation".into(),
            name: "Profile validation".into(),
            artist: "Profile validation".into(),
            country: "US".into(),
            url: format!("https://{}/event/profile-validation", profile.hosts[0]),
            mode: profile.mode,
            interval_secs: profile.interval_secs,
            notify_initial: profile.notify_initial,
            notify_offer_changes: profile.notify_offer_changes,
            min_quantity: profile.min_quantity,
            max_price_minor: profile.max_price_minor,
            currency: profile.currency.clone(),
            parser: profile.parser.clone(),
            ..Watch::default()
        };
        validate(&Config {
            watches: vec![synthetic],
            ..Config::default()
        })?;
    }
    let mut subscription_ids = HashSet::new();
    for subscription in &config.subscriptions {
        let artist_scope_valid = if subscription.all_artists {
            subscription.attraction_id.is_empty()
                && subscription.country.is_some()
                && (subscription.artist.is_empty() || bounded_label(&subscription.artist, 500))
        } else {
            bounded_label(&subscription.artist, 500) && safe_id(&subscription.attraction_id)
        };
        if !safe_id(&subscription.id)
            || !subscription_ids.insert(subscription.id.as_str())
            || !artist_scope_valid
            || !profile_ids.contains(subscription.profile.as_str())
        {
            bail!(
                "Subscription {} needs a unique id and existing profile; choose artist + attraction_id, or all_artists=true + country without an attraction_id",
                subscription.id
            );
        }
        if subscription
            .country
            .as_ref()
            .is_some_and(|v| v.len() != 2 || !v.bytes().all(|b| b.is_ascii_uppercase()))
            || subscription
                .city
                .as_ref()
                .is_some_and(|v| !bounded_label(v, 200))
            || subscription.exclude_keywords.len() > 100
            || subscription
                .exclude_keywords
                .iter()
                .any(|v| !bounded_label(v, 200))
        {
            bail!(
                "Subscription {} has invalid country, city, or excluded keywords",
                subscription.id
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_configuration() -> Config {
        Config {
            map_watches: vec![MapWatch {
                id: "korn-section-110".into(),
                name: "KoRn section 110".into(),
                url: "https://www.ticketmaster.cz/event/korn/1472650657".into(),
                section_id: "s_128".into(),
                ..MapWatch::default()
            }],
            ..Config::default()
        }
    }

    #[test]
    fn map_defaults_preserve_existing_configurations_without_requiring_credentials() {
        let config: Config = toml::from_str("database = 'data/old.sqlite'").unwrap();
        assert!(config.map_watches.is_empty());
        assert_eq!(config.map_database, Path::new("data/map-monitor.sqlite"));
        assert!(validate(&config).is_ok());
        let config = map_configuration();
        assert!(validate(&config).is_ok());
        assert!(config.discord.channel_id.is_none());
        assert_eq!(config.map_watches[0].limit, 20);
        assert_eq!(config.map_watches[0].interval_secs, 60);
        assert!(!config.map_watches[0].notify_initial);
        assert!(config.map_watches[0].notify_price_changes);
        assert!(!config.map_watches[0].notify_details_changes);
        assert!(toml::from_str::<MapWatch>("section_ids = 's_128'").is_err());
    }

    #[test]
    fn map_watches_bind_czech_events_sections_and_bounded_scan_parameters() {
        for url in [
            "https://www.ticketmaster.pl/event/korn/1472650657",
            "https://www.ticketmaster.cz/artist/1472650657",
            "https://www.ticketmaster.cz/event/korn/1472650657/checkout",
            "https://www.ticketmaster.cz/event/korn/1472650657?token=secret",
        ] {
            let mut config = map_configuration();
            config.map_watches[0].url = url.into();
            assert!(validate(&config).is_err(), "{url}");
        }
        for id in [
            "".to_owned(),
            "s_1] *".into(),
            "s_ě".into(),
            "x".repeat(129),
        ] {
            let mut config = map_configuration();
            config.map_watches[0].section_id = id;
            assert!(validate(&config).is_err());
        }
        for limit in [0, 51] {
            let mut config = map_configuration();
            config.map_watches[0].limit = limit;
            assert!(validate(&config).is_err());
        }
        for interval in [0, 31_536_001] {
            let mut config = map_configuration();
            config.map_watches[0].interval_secs = interval;
            assert!(validate(&config).is_err());
        }
        for name in [
            "".to_owned(),
            "   ".into(),
            "x".repeat(501),
            "bad\nname".into(),
        ] {
            let mut config = map_configuration();
            config.map_watches[0].name = name;
            assert!(validate(&config).is_err());
        }
        let mut config = map_configuration();
        config.map_watches[0].id = "x".repeat(101);
        assert!(validate(&config).is_err());
    }

    #[test]
    fn map_watch_identity_deduplicates_url_aliases_but_allows_different_sections() {
        let mut config = map_configuration();
        let mut second = config.map_watches[0].clone();
        second.id = "other".into();
        second.url = "https://ticketmaster.cz/event/another-slug/1472650657?language=en".into();
        config.map_watches.push(second);
        assert!(validate(&config).is_err());
        config.map_watches[1].section_id = "s_129".into();
        assert!(validate(&config).is_ok());
        config.map_watches[1].id = config.map_watches[0].id.clone();
        assert!(validate(&config).is_err());
        config.map_watches[1].id = "disabled-copy".into();
        config.map_watches[1].section_id = "s_128".into();
        config.map_watches[1].enabled = false;
        assert!(validate(&config).is_ok());
    }

    #[test]
    fn map_history_path_is_separate_after_lexical_and_config_relative_resolution() {
        let mut config = Config {
            database: "data/inventory.sqlite".into(),
            map_database: "./data/unused/../inventory.sqlite".into(),
            ..Config::default()
        };
        assert!(validate(&config).is_err());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        config.database = directory.path().join("inventory.sqlite");
        config.map_database = "inventory.sqlite".into();
        std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
        assert!(load(&path).is_err());
        config.map_database = "prices.sqlite".into();
        std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.map_database, directory.path().join("prices.sqlite"));
        assert_eq!(loaded.database, directory.path().join("inventory.sqlite"));
    }

    #[test]
    fn excludes_checkout_and_untrusted_hosts() {
        for url in [
            "https://secure.ticketmaster.pl/payment/?t=secret",
            "https://ticketmaster.pl.evil.test/event/1",
            "http://www.ticketmaster.pl/event/1",
            "https://www.ticketmaster.pl/event/1?token=secret",
            "https://name:secret@www.ticketmaster.pl/event/1",
        ] {
            assert!(validate_public_url(url).is_err());
        }
        assert!(
            validate_public_url("https://www.ticketmaster.pl/event/korn-bilety/496543017").is_ok()
        );
    }
    #[test]
    fn rejects_unknown_fields_to_catch_typos() {
        assert!(toml::from_str::<Config>("request_timeout_sec = 2").is_err());
    }

    fn hybrid_configuration() -> Config {
        Config {
            profiles: vec![WebsiteProfile {
                id: "poland".into(),
                hosts: vec!["www.ticketmaster.pl".into()],
                ..WebsiteProfile::default()
            }],
            subscriptions: vec![Subscription {
                id: "korn-pl".into(),
                artist: "KoRn".into(),
                attraction_id: "K8vArtist".into(),
                country: Some("PL".into()),
                profile: "poland".into(),
                ..Subscription::default()
            }],
            ..Config::default()
        }
    }

    #[test]
    fn hybrid_profiles_validate_scopes_and_references_without_claiming_live_selectors() {
        let mut config = hybrid_configuration();
        assert!(validate(&config).is_ok());
        assert!(!config.profiles[0].parser.selectors_verified);
        config.subscriptions[0].profile = "missing".into();
        assert!(validate(&config).is_err());
        config.subscriptions[0].profile = "poland".into();
        for host in [
            "secure.ticketmaster.pl",
            "www.ticketmaster.pl/event/123",
            "ticketmaster.pl.evil.test",
            "www.ticketmaster.pl:443",
        ] {
            config.profiles[0].hosts = vec![host.into()];
            assert!(validate(&config).is_err(), "{host}");
        }
    }

    #[test]
    fn hybrid_budget_and_filters_use_the_same_rules_as_manual_watches() {
        let mut config = hybrid_configuration();
        config.catalog.max_requests_per_day = 5001;
        assert!(validate(&config).is_err());
        config.catalog = CatalogConfig::default();
        config.profiles[0].max_price_minor = Some(10000);
        assert!(validate(&config).is_err());
        config.profiles[0].currency = Some("PLN".into());
        assert!(validate(&config).is_ok());
        config.profiles[0].parser.offers_selector = Some("[".into());
        assert!(validate(&config).is_err());
    }

    #[test]
    fn verified_offer_profiles_require_a_row_purchase_signal() {
        let mut config = hybrid_configuration();
        config.profiles[0].parser.offers_selector = Some(".offer".into());
        assert!(
            validate(&config).is_ok(),
            "incomplete unverified profiles can be developed"
        );
        config.profiles[0].parser.selectors_verified = true;
        assert!(
            validate(&config)
                .unwrap_err()
                .to_string()
                .contains("offer_available_selector")
        );
        config.profiles[0].parser.offer_available_selector = Some(".buy".into());
        assert!(validate(&config).is_ok());
        config.profiles[0].parser.offer_available_selector = Some("[".into());
        assert!(validate(&config).is_err());
    }

    #[test]
    fn generated_watch_namespace_cannot_be_claimed_manually() {
        let mut config = hybrid_configuration();
        config.watches.push(Watch {
            id: "auto-test".into(),
            name: "KoRn".into(),
            artist: "KoRn".into(),
            country: "PL".into(),
            url: "https://www.ticketmaster.pl/event/korn/123".into(),
            ..Watch::default()
        });
        assert!(
            validate(&config)
                .unwrap_err()
                .to_string()
                .contains("auto- is reserved")
        );
    }
}
