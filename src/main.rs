use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use ticket_watch::{
    catalog::{self, Snapshot},
    config::{self, Config, FetchMode, Watch},
    database::Db,
    discord::{self, Delivery, Discord},
    discovery::Discovery,
    fetch::Fetcher,
    model::{Availability, Observation},
    parser, profile,
    store::Store,
};
use tokio::{
    sync::{Notify, watch as updates},
    task::JoinSet,
};

#[derive(Parser)]
#[command(version, about = "Ticket website monitor; Discord sending is opt-in")]
struct Cli {
    #[arg(short, long, global = true, default_value = "config.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Monitor configured Czech sections; preview Discord messages unless sending is enabled.
    MapRun {
        #[arg(long)]
        once: bool,
        /// Send displayed-price observations to the configured Discord channel.
        #[arg(long)]
        send_discord: bool,
    },
    /// Show saved section-monitor health and pending notification counts.
    MapStatus,
    /// Inspect a reservation-attempt journal without creating it or performing actions.
    CartJournal {
        #[arg(long)]
        file: PathBuf,
    },
    /// Scan displayed resale prices in one Czech map section; never selects tickets.
    MapScan {
        #[arg(long)]
        url: String,
        #[arg(long)]
        section_id: String,
        /// Maximum visible resale candidates to inspect in this pass.
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=50))]
        limit: u32,
        #[arg(long)]
        headed: bool,
        /// Record observations and compare changes in a separate local SQLite file.
        #[arg(long)]
        state: Option<PathBuf>,
    },
    /// Inspect a regional map's references and rendered section outlines; never infers live seats.
    MapInspect {
        #[arg(long)]
        url: String,
        /// Read a saved capture; its transport and age remain unverified.
        #[arg(long, conflicts_with = "headed")]
        html: Option<PathBuf>,
        #[arg(long)]
        headed: bool,
        /// Save this map as a separate draft layout in the venue registry.
        #[arg(long)]
        save_layout: bool,
        /// Download the referenced public map image to a new file (also uses network with --html).
        #[arg(long)]
        map_image: Option<PathBuf>,
        /// Export observed section outlines as a rebuilt SVG; no source scripts or styling.
        #[arg(long)]
        geometry_svg: Option<PathBuf>,
        /// Open one Czech map section without selecting tickets (live browser only).
        #[arg(long, conflicts_with = "html")]
        section_id: Option<String>,
        /// Hover one rendered seat in that section without selecting it.
        #[arg(long, requires = "section_id", conflicts_with = "html")]
        seat_id: Option<String>,
    },
    /// Inspect configured Czech prices and quantity controls; never selects or reserves tickets.
    CartCatalog {
        #[arg(long)]
        url: String,
        /// Read a local capture; its transport and age remain unverified.
        #[arg(long, conflicts_with = "headed")]
        html: Option<PathBuf>,
        /// Show ordinary Chromium on an available graphical display.
        #[arg(long)]
        headed: bool,
    },
    /// Evaluate supplied group quotes offline; a decision never authorizes a reservation.
    CartPlan { file: PathBuf },
    /// Inspect any selected event using the built-in Ticketmaster adapter; no config or alerts.
    Inspect {
        #[arg(long)]
        url: String,
        /// Optional ISO currency when the page uses an ambiguous symbol.
        #[arg(long)]
        currency: Option<String>,
        /// Read a local capture instead of opening the event website.
        #[arg(long)]
        html: Option<PathBuf>,
        /// Show ordinary Chromium on an available graphical display.
        #[arg(long)]
        headed: bool,
    },
    /// Validate configuration without network access.
    Validate,
    /// List selected artists, locations and event watches.
    List,
    /// Refresh selected artists/locations into cached website watches; no alerts are sent.
    Sync,
    /// Inspect cached Discovery metadata, generated watches, and coverage gaps.
    Catalog,
    /// Manage reusable venue metadata and named layout references, without network access.
    Venues {
        #[command(subcommand)]
        action: VenueCommand,
    },
    /// Search artist IDs with your Ticketmaster Discovery key (metadata only).
    Artists {
        #[arg(long)]
        query: String,
        #[arg(long, default_value_t = 0)]
        page: u32,
    },
    /// Find matching events with Discovery. This does not establish live inventory.
    Events {
        #[arg(long)]
        attraction_id: String,
        #[arg(long)]
        country: Option<String>,
        #[arg(long)]
        city: Option<String>,
        #[arg(long, default_value_t = 0)]
        page: u32,
    },
    /// Inspect one page or saved HTML. Never sends a Discord message.
    Check {
        #[arg(long)]
        watch: String,
        #[arg(long)]
        html: Option<PathBuf>,
        /// Save fetched HTML locally for parser development. It may contain session data.
        #[arg(long)]
        save_html: Option<PathBuf>,
    },
    /// Review a manual watch's local capture and selector candidates; no network, state or alerts.
    ProfileCheck {
        #[arg(long)]
        watch: String,
        #[arg(long)]
        html: PathBuf,
        /// Optional capture metadata; otherwise uses <html>.json when present.
        #[arg(long)]
        capture_json: Option<PathBuf>,
    },
    /// Discover candidate event links; does not automatically enable watches.
    Discover {
        #[arg(long)]
        url: String,
        #[arg(long)]
        html: Option<PathBuf>,
        #[arg(long)]
        browser: bool,
    },
    /// Monitor configured watches. Defaults to preview-only delivery.
    Run {
        #[arg(long)]
        once: bool,
        /// Actually send queued alerts using the configured bot token and channel.
        #[arg(long)]
        send_discord: bool,
    },
    /// Replay saved HTML in an isolated in-memory database, without network access.
    Replay {
        #[arg(long)]
        watch: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Show saved watch health, last evidence and pending notifications.
    Status,
    /// Preview queued Discord payloads without delivering them.
    Outbox,
}

#[derive(Subcommand)]
enum VenueCommand {
    /// List saved venues. Uses config.database, or data/ticket.sqlite without a config.
    List,
    /// Import a reviewed JSON array atomically; never imports event inventory.
    Import { file: PathBuf },
    /// Show a venue and its independently named layout references.
    Show { id: String },
    /// Find an existing venue for another event using its venue metadata.
    Match {
        #[arg(long)]
        site: String,
        #[arg(long)]
        provider_id: Option<String>,
        #[arg(long)]
        name: String,
        #[arg(long)]
        city: String,
        #[arg(long)]
        country: String,
    },
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn find_watch(config: &Config, id: &str) -> Result<Watch> {
    if let Some(watch) = config.watches.iter().find(|w| w.id == id) {
        return Ok(watch.clone());
    }
    cached_watches(config)?
        .into_iter()
        .find(|w| w.id == id)
        .with_context(|| format!("Unknown watch: {id}; sync subscriptions first"))
}
fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
fn read_html(path: &Path, limit: usize) -> Result<String> {
    let file = std::fs::File::open(path).context("Open HTML fixture")?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("HTML fixture must be a regular file");
    }
    if metadata.len() > limit as u64 {
        bail!("HTML fixture exceeds configured body limit");
    }
    let mut input = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut input)
        .context("Read HTML fixture")?;
    if input.len() > limit {
        bail!("HTML fixture exceeds configured body limit");
    }
    String::from_utf8(input).context("HTML fixture must contain UTF-8 text")
}

fn write_new_artifact(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).context("Create map output directory")?;
    }
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .context("Create new map output file")?;
    if let Err(error) = output.write_all(bytes) {
        drop(output);
        let _ = std::fs::remove_file(path);
        return Err(error).context("Write map output file");
    }
    Ok(())
}

fn map_preset(
    snapshot: &ticket_watch::seat_map::SeatMapSnapshot,
) -> Result<ticket_watch::venues::VenuePreset> {
    let reference = snapshot
        .static_map
        .as_ref()
        .context("No static map reference to save")?;
    let preset = ticket_watch::venues::VenuePreset {
        site_host: snapshot.site_host.clone(),
        provider_venue_id: snapshot.venue.discovery_id.clone(),
        name: snapshot.venue.name.clone(),
        city: snapshot.venue.city.clone(),
        country: snapshot.venue.country.clone(),
        source_event_urls: vec![snapshot.event_url.clone()],
        layouts: vec![ticket_watch::venues::VenueLayout {
            id: reference.layout_fingerprint.clone(),
            name: "Observed static map".into(),
            adapter: Some("ticketmaster-static-map-v1".into()),
            map_url: Some(reference.url.clone()),
            sections: snapshot
                .configured_sections
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|section| section.title.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            source_event_urls: vec![snapshot.event_url.clone()],
            verified_at: None,
        }],
        ..Default::default()
    };
    preset.normalized()
}

fn save_map_preset(
    config: &Config,
    mut preset: ticket_watch::venues::VenuePreset,
) -> Result<ticket_watch::venues::VenuePreset> {
    let _lock = lock_database(&config.database)?;
    let mut store = Store::open(&config.database)?;
    if let Some(saved) = store.venue_lookup(&preset)? {
        preset.id = saved.id;
        if let Some(previous) = saved
            .layouts
            .iter()
            .find(|layout| layout.id == preset.layouts[0].id)
        {
            let next = &preset.layouts[0];
            if previous.map_url != next.map_url
                || previous.sections.iter().collect::<HashSet<_>>()
                    != next.sections.iter().collect::<HashSet<_>>()
            {
                bail!("Saved map layout conflicts with this observation; review the venue preset");
            }
            // Refreshing observed metadata must preserve an existing review and chosen name.
            let mut merged = previous.clone();
            for source in &next.source_event_urls {
                if !merged
                    .source_event_urls
                    .iter()
                    .any(|existing| ticket_watch::event_identity::same_event(existing, source))
                {
                    if merged.source_event_urls.len() >= 100 {
                        bail!("Saved map source history is full; review the venue preset");
                    }
                    merged.source_event_urls.push(source.clone());
                }
            }
            preset.layouts[0] = merged;
        }
    }
    store.venue_upsert(&preset)
}

/// Single-process ownership prevents duplicate polling and outbox delivery.
fn lock_database(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = path.with_extension("lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    file.try_lock()
        .context("Another monitor is already using this database")?;
    Ok(file)
}

async fn observe(fetcher: &Fetcher, watch: &Watch) -> (Observation, Option<u64>) {
    match fetcher.fetch(watch).await {
        Ok(page) => {
            eprintln!(
                "watch={} http_status={} elapsed_ms={} {}",
                watch.id, page.status, page.elapsed_ms, page.detail
            );
            let obs = page_observation(watch, &page, now());
            (obs, page.retry_after_secs)
        }
        Err(error) => (
            Observation::unknown(&watch.id, now(), format!("Fetch failed: {error}")),
            None,
        ),
    }
}

fn page_observation(
    watch: &Watch,
    page: &ticket_watch::fetch::FetchResult,
    at: i64,
) -> Observation {
    if matches!(page.status, 401 | 403 | 429) || page.adapter_state.as_deref() == Some("blocked") {
        let mut observation = Observation::unknown(
            &watch.id,
            at,
            format!("HTTP {}; {}", page.status, page.detail),
        );
        observation.availability = Availability::Blocked;
        return observation;
    }
    if !(200..300).contains(&page.status)
        || page
            .adapter_state
            .as_deref()
            .is_some_and(|state| state != "ready")
    {
        return Observation::unknown(
            &watch.id,
            at,
            format!("HTTP {}; {}", page.status, page.detail),
        );
    }
    parser::parse(watch, &page.html, at)
}

async fn dispatch_batch(
    db: &Db,
    config: &Config,
    sender: &mut Option<Discord>,
    shown: &mut HashSet<i64>,
) -> Result<()> {
    let cutoff = now() - config.alert_max_age_secs as i64;
    db.call(move |s| s.expire_before(cutoff)).await?;
    if let Some(sender) = sender.as_mut() {
        for _ in 0..32 {
            // Polling may cancel an older alert while Discord is busy. Re-read each send candidate.
            let at = now();
            let max_age = config.alert_max_age_secs as i64;
            let pending = db
                .call(move |s| {
                    s.expire_before(at - max_age)?;
                    if s.cooldown("discord:delivery")?
                        .is_some_and(|until| until > at)
                    {
                        return Ok(Vec::new());
                    }
                    s.pending(at, 1)
                })
                .await?;
            let Some(alert) = pending.into_iter().next() else {
                break;
            };
            let id = alert.id;
            match sender.send(&alert).await? {
                Delivery::Sent { message_id } => {
                    db.call(move |s| s.delivered(id, &message_id)).await?;
                    eprintln!("Discord accepted alert {id}");
                }
                Delivery::Retry { after_secs, reason } => {
                    let until = now().saturating_add(after_secs.max(1).min(i64::MAX as u64) as i64);
                    db.call(move |s| {
                        s.set_cooldown("discord:delivery", until)?;
                        s.retry(id, until, &reason)
                    })
                    .await?;
                    break;
                }
                Delivery::Permanent { reason } => {
                    let diagnostic = reason.clone();
                    let until = now() + 60;
                    db.call(move |s| s.retry(id, until, &reason)).await?;
                    bail!("Discord delivery stopped: {diagnostic}");
                }
            }
        }
    } else {
        let at = now();
        let pending = db.call(move |s| s.pending(at, 10_000)).await?;
        let still_pending: HashSet<i64> = pending.iter().map(|a| a.id).collect();
        shown.retain(|id| still_pending.contains(id));
        for alert in pending {
            if shown.insert(alert.id) {
                print_json(
                    &serde_json::json!({"mode":"preview_only", "outbox_id":alert.id,
                    "payload":discord::preview(&alert, &config.discord)}),
                )?;
            }
        }
    }
    Ok(())
}

async fn refresh_catalog(
    config: Config,
    db: Db,
    mut source: Discovery,
    updates: updates::Sender<Snapshot>,
) -> Result<()> {
    loop {
        let snapshot = catalog::refresh(&config, &db, &mut source).await?;
        let next = snapshot
            .next_refresh_at
            .unwrap_or(now() + config.catalog.refresh_interval_secs as i64);
        updates.send_replace(snapshot);
        tokio::time::sleep(Duration::from_secs(next.saturating_sub(now()).max(1) as u64)).await;
    }
}

fn cached_watches(config: &Config) -> Result<Vec<Watch>> {
    let value = if config.database.exists() {
        Store::open(&config.database)?.catalog_load()?
    } else {
        None
    };
    catalog::effective_watches(config, &catalog::decode(config, value)?)
}

async fn run(config: Config, once: bool, send: bool) -> Result<()> {
    let _lock = lock_database(&config.database)?;
    let has_subscriptions = config.subscriptions.iter().any(|s| s.enabled);
    let mut source = if has_subscriptions {
        Some(Discovery::from_env()?)
    } else {
        None
    };
    let mut sender = if send {
        Some(Discord::new(&config.discord)?)
    } else {
        None
    };
    let db = Db::open(&config.database)?;
    let delivery_config = config.discord.clone();
    db.call(move |s| s.reconcile_delivery(&delivery_config))
        .await?;
    let mut snapshot = catalog::load(&config, &db).await?;
    if once && let Some(source) = &mut source {
        snapshot = tokio::select! {
            _ = tokio::signal::ctrl_c() => { eprintln!("Stopped during catalog refresh"); return Ok(()); }
            result = catalog::refresh(&config, &db, source) => result?,
        };
    }
    let all = catalog::effective_watches(&config, &snapshot)?;
    let mut watches: Vec<Watch> = all.iter().filter(|w| w.enabled).cloned().collect();
    if watches.is_empty() && !has_subscriptions {
        bail!("No enabled watches or artist subscriptions");
    }
    db.call(move |s| s.reconcile(&all)).await?;
    let mut cooldowns = db.call(|s| s.cooldowns()).await?;
    cooldowns.retain(|scope, _| !scope.contains(':'));
    let fetcher = Arc::new(Fetcher::new(&config)?);
    let mut blocked_hosts: HashSet<String> = cooldowns
        .iter()
        .filter(|(_, until)| **until > now())
        .map(|(host, _)| host.clone())
        .collect();
    let mut host_due: HashMap<String, Instant> =
        cooldowns
            .into_iter()
            .map(|(host, until)| {
                (
                    host,
                    Instant::now()
                        + Duration::from_secs(
                            until.saturating_sub(now()).clamp(0, 86_400 * 365) as u64
                        ),
                )
            })
            .collect();
    let mut due: HashMap<String, Instant> = watches
        .iter()
        .map(|w| (w.id.clone(), Instant::now()))
        .collect();
    let mut busy = HashSet::new();
    let mut done = HashSet::new();
    let mut jobs = JoinSet::new();
    let notify = Arc::new(Notify::new());
    let mut shown = HashSet::new();
    let (catalog_tx, mut catalog_rx) = updates::channel(snapshot);
    let mut catalog_task = if !once {
        source.map(|source| {
            tokio::spawn(refresh_catalog(
                config.clone(),
                db.clone(),
                source,
                catalog_tx,
            ))
        })
    } else {
        None
    };
    let mut dispatch_task = if once {
        None
    } else {
        let dispatch_db = db.clone();
        let dispatch_config = config.clone();
        let wake = notify.clone();
        let mut task_sender = sender.take();
        Some(tokio::spawn(async move {
            let mut seen = HashSet::new();
            loop {
                dispatch_batch(&dispatch_db, &dispatch_config, &mut task_sender, &mut seen).await?;
                tokio::select! { _ = wake.notified() => {}, _ = tokio::time::sleep(Duration::from_secs(1)) => {} }
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        }))
    };
    eprintln!(
        "Monitoring {} cached/manual event(s), {} subscription(s); Discord: {}. Ctrl+C stops.",
        watches.len(),
        config.subscriptions.iter().filter(|s| s.enabled).count(),
        if send { "send" } else { "preview only" }
    );
    let mut interrupted = false;
    let outcome: Result<()> = async {
        loop {
            if let Some(task) = &mut dispatch_task && task.is_finished() {
                task.await.context("Discord worker failed")??; bail!("Discord worker stopped");
            }
            if let Some(task) = &mut catalog_task && task.is_finished() {
                task.await.context("Catalog worker failed")??; bail!("Catalog worker stopped");
            }
            let mut order: Vec<usize> = (0..watches.len()).collect();
            order.sort_by_key(|index| due[&watches[*index].id]);
            for index in order {
                let watch = &watches[index];
                if jobs.len() >= config.max_in_flight { break; }
                if busy.contains(&watch.id) || (once && done.contains(&watch.id)) || due[&watch.id] > Instant::now() { continue; }
                let host = config::validate_public_url(&watch.url)?.host_str().unwrap().to_owned();
                if host_due.get(&host).is_some_and(|at| *at > Instant::now()) {
                    if once && blocked_hosts.contains(&host) {
                        eprintln!("watch={} skipped: host cooldown active", watch.id);
                        done.insert(watch.id.clone());
                    }
                    continue;
                }
                let until = now().saturating_add(config.per_host_interval_secs as i64);
                let saved_host = host.clone();
                db.call(move |s| s.set_cooldown(&saved_host, until)).await?;
                host_due.insert(host.clone(), Instant::now() + Duration::from_secs(config.per_host_interval_secs));
                due.insert(watch.id.clone(), Instant::now() + Duration::from_secs(watch.interval_secs));
                busy.insert(watch.id.clone());
                let w = watch.clone(); let f = fetcher.clone();
                jobs.spawn(async move { let (obs, retry) = observe(&f, &w).await; (w, host, obs, retry) });
            }
            if once && done.len() == watches.len() && jobs.is_empty() { break; }
            tokio::select! {
                _ = tokio::signal::ctrl_c() => { interrupted = true; eprintln!("Stopping monitor"); break; }
                changed = catalog_rx.changed(), if catalog_task.is_some() => {
                    changed.context("Catalog updates stopped")?;
                    let snapshot = catalog_rx.borrow_and_update().clone();
                    let all = catalog::effective_watches(&config, &snapshot)?;
                    let next: Vec<_> = all.iter().filter(|w|w.enabled).cloned().collect();
                    for watch in &next {
                        let unchanged = watches.iter().find(|old|old.id == watch.id)
                            .is_some_and(|old| serde_json::to_value(old).ok() == serde_json::to_value(watch).ok());
                        if !unchanged { due.insert(watch.id.clone(), Instant::now()); }
                    }
                    due.retain(|id,_|next.iter().any(|w| &w.id == id));
                    db.call(move |s|s.reconcile(&all)).await?;
                    watches = next;
                    eprintln!("Catalog updated: {} monitored events, {} API requests, {} incomplete selections; inspect with catalog",
                        watches.len(), snapshot.requests_attempted, snapshot.selections.iter().filter(|s|!s.complete).count());
                }
                result = jobs.join_next(), if !jobs.is_empty() => {
                    let (watch, host, obs, retry) = result.context("Fetch task vanished")?.context("Fetch task failed")?;
                    busy.remove(&watch.id);
                    if once { done.insert(watch.id.clone()); }
                    let mut wait = watch.interval_secs;
                    if obs.availability == Availability::Blocked {
                        wait = wait.max(config.blocked_retry_secs).max(retry.unwrap_or(0));
                        let until = now().saturating_add(wait.min(i64::MAX as u64) as i64);
                        let saved_host = host.clone();
                        blocked_hosts.insert(host.clone());
                        db.call(move |s| s.set_cooldown(&saved_host, until)).await?;
                        host_due.insert(host.clone(), Instant::now() + Duration::from_secs(wait.min(86_400 * 365)));
                    } else if obs.availability == Availability::Unknown {
                        wait = wait.max(config.unknown_retry_secs).max(retry.unwrap_or(0));
                    }
                    if let Some(seconds) = retry.filter(|s| *s > 0) {
                        let until = now().saturating_add(seconds.min(i64::MAX as u64) as i64);
                        let saved_host = host.clone();
                        db.call(move |s| s.set_cooldown(&saved_host, until)).await?;
                        blocked_hosts.insert(host.clone());
                        let at = Instant::now() + Duration::from_secs(seconds.min(86_400 * 365));
                        host_due.entry(host).and_modify(|old| *old = (*old).max(at)).or_insert(at);
                    }
                    // A catalog refresh may retire/edit a watch during its fetch. Keep transport backoff,
                    // but discard inventory from that obsolete configuration before touching its baseline.
                    let current = watches.iter().find(|current| current.id == watch.id);
                    if !current.is_some_and(|current| serde_json::to_value(current).ok() == serde_json::to_value(&watch).ok()) { continue; }
                    if wait != watch.interval_secs || due[&watch.id] <= Instant::now() {
                        due.insert(watch.id.clone(), Instant::now() + Duration::from_secs(wait.min(86_400 * 365)));
                    }
                    let summary = serde_json::json!({"watch_id":obs.watch_id,
                        "observed_at":obs.observed_at,"availability":obs.availability,
                        "evidence":obs.evidence,"offer_count":obs.offers.len(),"detail":obs.detail});
                    let count = db.call(move |s| s.apply(&watch, &obs)).await?;
                    if count > 0 { notify.notify_one(); }
                    print_json(&summary)?;
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        }
        Ok(())
    }.await;
    if let Some(task) = dispatch_task {
        task.abort();
        let _ = task.await;
    }
    if let Some(task) = catalog_task {
        task.abort();
        let _ = task.await;
    }
    jobs.abort_all();
    while jobs.join_next().await.is_some() {}
    fetcher.shutdown().await;
    outcome?;
    if once && !interrupted {
        dispatch_batch(&db, &config, &mut sender, &mut shown).await?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = match &cli.command {
        Command::MapInspect { save_layout, .. }
            if !save_layout || (cli.config == Path::new("config.toml") && !cli.config.exists()) =>
        {
            Config::default()
        }
        Command::Artists { .. }
        | Command::MapScan { .. }
        | Command::Events { .. }
        | Command::Inspect { .. }
        | Command::CartCatalog { .. }
        | Command::CartJournal { .. }
        | Command::CartPlan { .. } => Config::default(),
        Command::Venues { .. }
            if cli.config == Path::new("config.toml") && !cli.config.exists() =>
        {
            Config::default()
        }
        _ => config::load(&cli.config)?,
    };
    match cli.command {
        Command::MapRun { once, send_discord } => {
            ticket_watch::map_monitor::run(config, once, send_discord).await?;
        }
        Command::MapStatus => {
            if !config.map_database.is_file() {
                bail!("Map monitor database does not exist; run map-run first");
            }
            let store =
                ticket_watch::map_monitor_store::MapMonitorStore::open(&config.map_database)?;
            print_json(&serde_json::json!({
                "watches": store.status()?,
                "cooldowns": {
                    "ticketmaster_cz_until_ms": store.cooldown("host:ticketmaster.cz")?,
                    "discord_until_ms": store.cooldown("discord:delivery")?
                }
            }))?;
        }
        Command::CartJournal { file } => {
            print_json(&ticket_watch::cart_journal::inspect_file(&file)?)?;
        }
        Command::MapScan {
            url,
            section_id,
            limit,
            headed,
            state,
        } => {
            let mut browser_config = config.clone();
            browser_config.browser.headed = headed;
            browser_config.request_timeout_secs = 45;
            let fetcher = Fetcher::new(&browser_config)?;
            let result = fetcher.fetch_map_scan(&url, &section_id, limit).await;
            fetcher.shutdown().await;
            let page = result?;
            if !(200..300).contains(&page.status) || page.adapter_state.as_deref() != Some("ready")
            {
                bail!(
                    "HTTP {}; {}; no map scan extracted",
                    page.status,
                    page.detail
                );
            }
            let raw = page.map_scan.context("No captured map scan evidence")?;
            let snapshot =
                ticket_watch::seat_map::extract_seat_map(&url, &page.final_url, &page.html)?;
            let scan = ticket_watch::map_scan::validate_scan(
                &snapshot,
                &raw,
                chrono::Utc::now().timestamp_millis(),
            )?;
            let state_changes = state
                .map(|path| ticket_watch::map_scan_store::MapScanStore::open(&path)?.record(&scan))
                .transpose()?;
            print_json(&serde_json::json!({
                "source":"live_browser", "transport_binding":"observed_main_document",
                "elapsed_ms":page.elapsed_ms, "scan":scan, "state_changes":state_changes
            }))?;
        }
        Command::MapInspect {
            url,
            html,
            headed,
            save_layout,
            map_image,
            geometry_svg,
            section_id,
            seat_id,
        } => {
            config::validate_public_url(&url)?;
            if !ticket_watch::event_identity::same_event(&url, &url) {
                bail!("Use a supported public Ticketmaster event URL");
            }
            let (body, final_url, source, transport_binding, map_inspection) =
                if let Some(path) = html {
                    (
                        read_html(&path, config.max_body_bytes)?,
                        url.clone(),
                        "saved_html",
                        "unverified_saved_html",
                        None,
                    )
                } else {
                    let mut browser_config = config.clone();
                    browser_config.browser.headed = headed;
                    browser_config.request_timeout_secs = 45;
                    let fetcher = Fetcher::new(&browser_config)?;
                    let result = if let Some(section_id) = section_id.as_deref() {
                        fetcher
                            .fetch_map_section(&url, section_id, seat_id.as_deref())
                            .await
                    } else {
                        fetcher.fetch_map(&url).await
                    };
                    fetcher.shutdown().await;
                    let page = result?;
                    if !(200..300).contains(&page.status)
                        || page.adapter_state.as_deref() == Some("blocked")
                    {
                        bail!("HTTP {}; {}; no map extracted", page.status, page.detail);
                    }
                    (
                        page.html,
                        page.final_url,
                        "live_browser",
                        "observed_main_document",
                        page.map_inspection,
                    )
                };
            let snapshot = ticket_watch::seat_map::extract_seat_map(&url, &final_url, &body)?;
            let section_inspection = match (section_id.as_deref(), map_inspection.as_ref()) {
                (Some(section_id), Some(inspection)) => {
                    if inspection.section_id != section_id
                        || inspection.seat_id.as_deref() != seat_id.as_deref()
                    {
                        bail!("Map section inspection target mismatch");
                    }
                    Some(ticket_watch::map_inspection::validate_inspection(
                        &snapshot, inspection,
                    )?)
                }
                (Some(_), None) => bail!("No captured evidence for the requested map section"),
                (None, Some(_)) => bail!("Unsolicited map section inspection"),
                (None, None) => None,
            };
            // Validate a save candidate before attempting either optional output action.
            let preset = save_layout.then(|| map_preset(&snapshot)).transpose()?;
            let svg = geometry_svg
                .as_ref()
                .map(|_| {
                    ticket_watch::map_geometry::render_svg(
                        snapshot
                            .rendered_geometry
                            .as_ref()
                            .context("No captured section geometry to export")?,
                    )
                })
                .transpose()?;
            for path in [map_image.as_ref(), geometry_svg.as_ref()]
                .into_iter()
                .flatten()
            {
                if path.exists() {
                    bail!("Map output already exists; choose a new path");
                }
            }
            if map_image.is_some() && map_image == geometry_svg {
                bail!("Map image and geometry SVG need different output paths");
            }
            let image = if let Some(path) = map_image {
                let reference = snapshot
                    .static_map
                    .as_ref()
                    .context("No static map image to download")?;
                if path.exists() {
                    bail!("Map image output already exists; choose a new path");
                }
                let asset = ticket_watch::map_asset::download_map(&reference.url).await?;
                write_new_artifact(&path, &asset.bytes)?;
                Some(
                    serde_json::json!({"path":path, "bytes":asset.bytes.len(), "content_type":asset.content_type,
                    "sha256":asset.sha256, "validation":"mime_and_signature", "geometry_verified":false}),
                )
            } else {
                None
            };
            let exported_geometry = if let (Some(path), Some(svg)) = (geometry_svg, svg) {
                let result = write_new_artifact(&path, svg.as_bytes());
                if image.is_some() {
                    result.context("Map image was saved; geometry export failed")?;
                } else {
                    result?;
                }
                Some(
                    serde_json::json!({"path":path,"format":"sanitized_svg","stock_verified":false,"adjacency_verified":false}),
                )
            } else {
                None
            };
            let saved = preset
                .map(|preset| save_map_preset(&config, preset))
                .transpose();
            let saved = if image.is_some() || exported_geometry.is_some() {
                saved.context("Map output was saved; venue layout save failed")?
            } else {
                saved?
            };
            print_json(
                &serde_json::json!({"source":source, "transport_binding":transport_binding,
                "map":snapshot, "saved_venue":saved, "downloaded_image":image, "exported_geometry":exported_geometry,
                "section_inspection":section_inspection}),
            )?;
        }
        Command::CartPlan { file } => {
            let mut input = Vec::new();
            std::fs::File::open(file)
                .context("Open cart quote request")?
                .take(ticket_watch::cart::MAX_INPUT_BYTES as u64 + 1)
                .read_to_end(&mut input)
                .context("Read cart quote request")?;
            print_json(&ticket_watch::cart::plan_json(
                &input,
                chrono::Utc::now().timestamp_millis(),
            ))?;
        }
        Command::CartCatalog { url, html, headed } => {
            let parsed = config::validate_public_url(&url)
                .context("Use a public Ticketmaster Czech event URL")?;
            if !matches!(
                parsed.host_str(),
                Some("ticketmaster.cz" | "www.ticketmaster.cz")
            ) || !ticket_watch::event_identity::same_event(&url, &url)
            {
                bail!("Use a public Ticketmaster Czech event URL");
            }
            let (body, final_url, source, transport_binding) = if let Some(path) = html {
                (
                    read_html(&path, config.max_body_bytes)?,
                    url.clone(),
                    "saved_html",
                    "unverified_saved_html",
                )
            } else {
                let mut config = config;
                config.browser.headed = headed;
                config.request_timeout_secs = 45;
                let fetcher = Fetcher::new(&config)?;
                let result = fetcher
                    .fetch(&Watch {
                        id: "cart-catalog".into(),
                        url: url.clone(),
                        mode: FetchMode::Browser,
                        parser: config::ParserConfig {
                            adapter: config::PageAdapter::Ticketmaster,
                            ..Default::default()
                        },
                        ..Watch::default()
                    })
                    .await;
                fetcher.shutdown().await;
                let page = result?;
                if !(200..300).contains(&page.status)
                    || page.adapter_state.as_deref() == Some("blocked")
                {
                    bail!(
                        "HTTP {}; {}; no cart catalog extracted",
                        page.status,
                        page.detail
                    );
                }
                (
                    page.html,
                    page.final_url,
                    "live_browser",
                    "observed_main_document",
                )
            };
            let catalog =
                ticket_watch::cart_catalog::extract_cz_cart_catalog(&url, &final_url, &body)?;
            print_json(&serde_json::json!({
                "source": source,
                "transport_binding": transport_binding,
                "reservation_attempted": false,
                "catalog": catalog,
            }))?;
        }
        Command::Venues { action } => {
            let _lock = if matches!(action, VenueCommand::Import { .. }) {
                Some(lock_database(&config.database)?)
            } else {
                None
            };
            let mut store = Store::open(&config.database)?;
            match action {
                VenueCommand::List => print_json(&store.venue_list()?)?,
                VenueCommand::Import { file } => {
                    if std::fs::metadata(&file)?.len() > 8 * 1024 * 1024 {
                        bail!("Venue import exceeds 8 MiB");
                    }
                    let presets: Vec<ticket_watch::venues::VenuePreset> =
                        serde_json::from_slice(&std::fs::read(file)?)
                            .context("Read venue preset array")?;
                    print_json(&store.venue_import(&presets)?)?;
                }
                VenueCommand::Show { id } => {
                    print_json(&store.venue_get(&id)?.context("Unknown venue preset")?)?;
                }
                VenueCommand::Match {
                    site,
                    provider_id,
                    name,
                    city,
                    country,
                } => {
                    let query = ticket_watch::venues::VenuePreset {
                        id: String::new(),
                        site_host: site,
                        provider_venue_id: provider_id,
                        name,
                        city,
                        country,
                        layouts: vec![],
                        source_event_urls: vec![],
                    };
                    print_json(&store.venue_lookup(&query)?)?;
                }
            }
        }
        Command::Inspect {
            url,
            currency,
            html,
            headed,
        } => {
            config::validate_public_url(&url)?;
            if !ticket_watch::event_identity::same_event(&url, &url) {
                bail!("Use a supported public Ticketmaster event URL");
            }
            if currency.as_ref().is_some_and(|value| {
                value.len() != 3 || !value.bytes().all(|b| b.is_ascii_uppercase())
            }) {
                bail!("Currency must be a three-letter uppercase ISO code");
            }
            let watch = Watch {
                id: "inspection".into(),
                url,
                currency,
                mode: FetchMode::Browser,
                parser: config::ParserConfig {
                    adapter: config::PageAdapter::Ticketmaster,
                    ..Default::default()
                },
                ..Watch::default()
            };
            let (source, observation) = if let Some(path) = html {
                (
                    "saved_html",
                    parser::parse(&watch, &read_html(&path, config.max_body_bytes)?, 0),
                )
            } else {
                let mut config = config;
                config.browser.headed = headed;
                config.request_timeout_secs = 45;
                let fetcher = Fetcher::new(&config)?;
                let (observation, _) = observe(&fetcher, &watch).await;
                fetcher.shutdown().await;
                ("live_browser", observation)
            };
            print_json(
                &serde_json::json!({"source":source,"event_url":watch.url,"observation":observation}),
            )?;
        }
        Command::Validate => {
            println!(
                "Configuration valid: {} manual watches, {} subscriptions, {} map watches; default mode is preview-only",
                config.watches.len(),
                config.subscriptions.len(),
                config.map_watches.len()
            );
        }
        Command::List => {
            for w in cached_watches(&config)? {
                print_json(
                    &serde_json::json!({"id":w.id,"name":w.name,"artist":w.artist,"country":w.country,"city":w.city,"enabled":w.enabled,"mode":w.mode,"interval_secs":w.interval_secs,"url":w.url}),
                )?;
            }
        }
        Command::Sync => {
            let _lock = lock_database(&config.database)?;
            let db = Db::open(&config.database)?;
            let snapshot = catalog::refresh(&config, &db, &mut Discovery::from_env()?).await?;
            let watches = catalog::effective_watches(&config, &snapshot)?;
            db.call(move |s| s.reconcile(&watches)).await?;
            print_json(&snapshot)?;
        }
        Command::Catalog => {
            let store = Store::open(&config.database)?;
            let mut snapshot = catalog::decode(&config, store.catalog_load()?)?;
            snapshot.next_refresh_at = Some(
                store
                    .cooldown("catalog:refresh")?
                    .unwrap_or(0)
                    .max(store.cooldown("discovery:api")?.unwrap_or(0)),
            );
            print_json(&snapshot)?;
        }
        Command::Artists { query, page } => {
            print_json(&Discovery::from_env()?.artists_page(&query, page).await?)?
        }
        Command::Events {
            attraction_id,
            country,
            city,
            page,
        } => {
            print_json(
                &Discovery::from_env()?
                    .events_page(&attraction_id, country.as_deref(), city.as_deref(), page)
                    .await?,
            )?;
        }
        Command::Check {
            watch,
            html,
            save_html,
        } => {
            let watch = find_watch(&config, &watch)?;
            if let Some(path) = html {
                print_json(&parser::parse(
                    &watch,
                    &read_html(&path, config.max_body_bytes)?,
                    now(),
                ))?;
            } else if let Some(path) = save_html {
                let fetcher = Fetcher::new(&config)?;
                let page = fetcher.fetch(&watch).await?;
                if !(200..300).contains(&page.status) {
                    bail!("HTTP {}; {}; capture not saved", page.status, page.detail);
                }
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, &page.html)?;
                eprintln!("Saved local page capture; review/redact before sharing");
                print_json(&page_observation(&watch, &page, now()))?;
            } else {
                let (obs, _) = observe(&Fetcher::new(&config)?, &watch).await;
                print_json(&obs)?;
            }
        }
        Command::ProfileCheck {
            watch,
            html,
            capture_json,
        } => {
            // Do not call find_watch here: resolving a generated watch can open/migrate SQLite.
            let watch = config.watches.iter().find(|candidate| candidate.id == watch)
                .context("Profile review needs a manual watch in this configuration; copy the event into a manual review entry first")?;
            let body = read_html(&html, config.max_body_bytes)?;
            let metadata_path = capture_json.or_else(|| {
                let mut sidecar = html.as_os_str().to_os_string();
                sidecar.push(".json");
                let path = PathBuf::from(sidecar);
                path.exists().then_some(path)
            });
            let metadata = metadata_path
                .as_ref()
                .map(|path| read_html(path, profile::MAX_CAPTURE_JSON_BYTES))
                .transpose()?;
            print_json(&profile::review(watch, &body, metadata.as_deref())?)?;
        }
        Command::Discover { url, html, browser } => {
            config::validate_public_url(&url)?;
            let body = if let Some(path) = html {
                read_html(&path, config.max_body_bytes)?
            } else {
                let page = Fetcher::new(&config)?
                    .fetch_url(
                        &url,
                        if browser {
                            FetchMode::Browser
                        } else {
                            FetchMode::Http
                        },
                    )
                    .await?;
                if !(200..300).contains(&page.status) {
                    bail!("Discovery HTTP {}; {}", page.status, page.detail);
                }
                page.html
            };
            print_json(&parser::discover(&body, &url)?)?;
        }
        Command::Run { once, send_discord } => run(config, once, send_discord).await?,
        Command::Replay { watch, files } => {
            let watch = find_watch(&config, &watch)?;
            let mut store = Store::open(Path::new(":memory:"))?;
            store.reconcile(std::slice::from_ref(&watch))?;
            for (step, path) in files.into_iter().enumerate() {
                let obs = parser::parse(
                    &watch,
                    &read_html(&path, config.max_body_bytes)?,
                    now() + step as i64,
                );
                let added = store.apply(&watch, &obs)?;
                print_json(&serde_json::json!({"file":path,"new_alerts":added,"observation":obs}))?;
                for pending in store.pending(i64::MAX, 100)? {
                    print_json(
                        &serde_json::json!({"mode":"offline_preview", "payload":discord::preview(&pending, &config.discord)}),
                    )?;
                    store.delivered(pending.id, "offline-preview")?;
                }
            }
        }
        Command::Status => print_json(&Store::open(&config.database)?.status()?)?,
        Command::Outbox => {
            for pending in Store::open(&config.database)?.pending(i64::MAX, 100)? {
                print_json(
                    &serde_json::json!({"outbox_id":pending.id,"payload":discord::preview(&pending, &config.discord)}),
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod observation_tests {
    use super::*;

    #[test]
    fn browser_restriction_state_overrides_even_readable_offer_markup() {
        let mut watch = Watch::default();
        watch.parser.adapter = config::PageAdapter::Ticketmaster;
        for (state, expected) in [
            ("blocked", Availability::Blocked),
            ("unsupported", Availability::Unknown),
            ("no_offers", Availability::Unknown),
            ("ready", Availability::Available),
        ] {
            let page = ticket_watch::fetch::FetchResult {
                status: 200,
                html: include_str!("../tests/fixtures/ticketmaster-regional.html").into(),
                retry_after_secs: None,
                elapsed_ms: 1,
                final_url: String::new(),
                detail: "browser result".into(),
                adapter_state: Some(state.into()),
                map_inspection: None,
                map_scan: None,
            };
            let result = page_observation(&watch, &page, 10);
            assert_eq!(result.availability, expected, "{state}");
            assert_eq!(result.offers.len(), if state == "ready" { 2 } else { 0 });
        }
    }
}
