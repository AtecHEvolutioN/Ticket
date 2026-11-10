use ticket_watch::{
    config::{Config, DiscordConfig},
    discord,
    model::{Availability, InventoryKind},
    parser,
    store::Store,
};

#[test]
fn mixed_sale_channels_survive_parsing_restart_and_discord_rendering() {
    let config: Config = toml::from_str(include_str!("../examples/offline-demo.toml")).unwrap();
    let watch = &config.watches[0];
    let mixed = parser::parse(
        watch,
        include_str!("fixtures/synthetic-mixed-sales.html"),
        2,
    );
    assert_eq!(mixed.availability, Availability::Available);
    assert_eq!(mixed.offers.len(), 3);
    for (ticket_type, expected) in [
        ("Standard Ticket", InventoryKind::Primary),
        ("Official Platinum Ticket", InventoryKind::Resale),
        ("VIP Ticket", InventoryKind::Unknown),
    ] {
        assert_eq!(
            mixed
                .offers
                .iter()
                .find(|offer| offer.ticket_type.as_deref() == Some(ticket_type))
                .unwrap()
                .inventory_kind,
            expected
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("offline.sqlite");
    let mut store = Store::open(&path).unwrap();
    let unavailable = parser::parse(
        watch,
        include_str!("fixtures/synthetic-unavailable.html"),
        1,
    );
    assert_eq!(store.apply(watch, &unavailable).unwrap(), 0);
    assert_eq!(store.apply(watch, &mixed).unwrap(), 1);
    drop(store);

    let mut store = Store::open(&path).unwrap();
    let alerts = store.pending(2, 10).unwrap();
    assert_eq!(alerts.len(), 1);
    let message = discord::preview(&alerts[0], &DiscordConfig::default());
    let fields = message["embeds"][0]["fields"].as_array().unwrap();
    let values = fields
        .iter()
        .map(|field| field["value"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(
        values
            .iter()
            .any(|value| value.contains("Sale type: Original (primary sale)")
                && value.contains("Standard Ticket")
                && value.contains("199,00 zł"))
    );
    assert!(
        values
            .iter()
            .any(|value| value.contains("Sale type: Resale")
                && value.contains("Official Platinum Ticket")
                && value.contains("249,00 zł"))
    );
    assert!(
        values
            .iter()
            .any(|value| value.contains("Sale type: Unknown")
                && value.contains("VIP Ticket")
                && value.contains("299,00 zł"))
    );
    assert_eq!(
        store
            .apply(
                watch,
                &parser::parse(
                    watch,
                    include_str!("fixtures/synthetic-mixed-sales.html"),
                    3
                )
            )
            .unwrap(),
        0
    );
}
