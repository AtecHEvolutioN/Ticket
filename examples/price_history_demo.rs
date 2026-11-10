//! Offline end-to-end fixture: parser -> watcher database -> JSON/HTML chart.
//! Usage: cargo run --example price_history_demo -- /tmp/ticket-history-demo
use anyhow::{Context, Result, ensure};
use std::path::PathBuf;
use ticket_watch::{
    config::Config,
    market_chart,
    model::{Availability, Evidence, FeeScope, PriceScope, QuantityKind},
    parser,
    store::Store,
};

fn main() -> Result<()> {
    let directory = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("Supply a new output directory")?,
    );
    ensure!(
        !directory.exists(),
        "Choose a new demo directory; existing data is never overwritten"
    );
    std::fs::create_dir(&directory)?;
    let mut config: Config = toml::from_str(include_str!("offline-demo.toml"))?;
    let watch = &mut config.watches[0];
    watch.name = "SYNTHETIC DEMO — KoRn".into();
    watch.notify_initial = false;
    let database = directory.join("synthetic.sqlite");
    let mut store = Store::open(&database)?;
    let generated_at = chrono::Utc::now().timestamp();
    let start = generated_at - 6 * 3600;
    for step in 0..=360 {
        let mut observation = parser::parse(
            watch,
            include_str!("../tests/fixtures/synthetic-mixed-sales.html"),
            start + step * 60,
        );
        for (index, offer) in observation.offers.iter_mut().enumerate() {
            offer.price_minor = Some(19_900 + index as i64 * 5_000 + (step % 31) * 90 - step * 12);
            offer.price_scope = PriceScope::PerTicket;
            offer.fees = FeeScope::Included;
            offer.quantity = Some(2 + (step % 5) as u32);
            offer.quantity_kind = QuantityKind::Listing;
        }
        if (100..115).contains(&step) {
            observation.availability = Availability::Blocked;
            observation.evidence = Evidence::None;
            observation.offers.clear();
            observation.adapter = None;
            observation.complete = false;
        }
        store.apply(watch, &observation)?;
    }
    drop(store);
    for extension in ["json", "html"] {
        let report = market_chart::export(
            std::slice::from_ref(&database),
            &directory.join(format!("synthetic-history.{extension}")),
            None,
            None,
            None,
            generated_at,
        )?;
        println!("{}", serde_json::to_string(&report)?);
    }
    Ok(())
}
