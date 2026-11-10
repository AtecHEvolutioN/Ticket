//! Sale-channel labels are evidence only in an explicitly selected inventory
//! field or reviewed provider component. Do not apply this classifier to an
//! event title, price, seating section or general ticket-type description.
use crate::model::InventoryKind;

/// Recognize a complete, explicit primary/resale label. Mixed and unfamiliar
/// wording stays unknown; Standard, VIP and Platinum do not identify a seller.
pub fn classify_inventory_label(raw: &str) -> InventoryKind {
    if raw.len() > 512
        || raw
            .chars()
            .any(|character| character.is_control() && !character.is_whitespace())
    {
        return InventoryKind::Unknown;
    }
    let label = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    match label.as_str() {
        "primary" | "primary tickets" | "primary sale" | "original" | "original tickets"
        | "original sale" => InventoryKind::Primary,
        "resale"
        | "resale tickets"
        | "tickets resale"
        | "resale-tickets"
        | "verified resale ticket"
        | "verified resale tickets"
        | "bilety resale"
        | "ověřená přeprodávaná vstupenka"
        | "ověřené přeprodávané vstupenky" => InventoryKind::Resale,
        _ => InventoryKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Offer, TicketCategory};

    #[test]
    fn explicit_inventory_labels_are_normalized_but_ticket_types_and_mixed_labels_are_unknown() {
        for label in ["PRIMARY", "  Original tickets  ", "Primary sale"] {
            assert_eq!(classify_inventory_label(label), InventoryKind::Primary);
        }
        for label in [
            "Resale",
            " Verified\tResale Ticket ",
            "Bilety Resale",
            "Ověřená přeprodávaná vstupenka",
            "Ověřené přeprodávané vstupenky",
        ] {
            assert_eq!(classify_inventory_label(label), InventoryKind::Resale);
        }
        for label in [
            "",
            "Standard",
            "Standard Ticket",
            "VIP",
            "Official Platinum",
            "Reguliere verkoop",
            "Primary / Resale",
            "Original and Resale tickets",
            "Non-resale",
            "Available",
            "Sold out",
            "$200",
            "Resale\0",
        ] {
            assert_eq!(
                classify_inventory_label(label),
                InventoryKind::Unknown,
                "{label}"
            );
        }
        assert_eq!(
            classify_inventory_label(&"Resale ".repeat(100)),
            InventoryKind::Unknown
        );
    }

    #[test]
    fn legacy_observations_do_not_acquire_unverified_provenance_from_raw_text() {
        let offer: Offer = serde_json::from_value(serde_json::json!({"key":"legacy","inventory":"Resale","ticket_type":"Official Platinum"})).unwrap();
        assert_eq!(offer.inventory_kind, InventoryKind::Unknown);
        assert_eq!(offer.inventory.as_deref(), Some("Resale"));
        let category: TicketCategory = serde_json::from_value(serde_json::json!({"key":"legacy","ticket_type":"Standard","price_text":null,"price_min_minor":null,"price_max_minor":null,"currency":null,"selectable_quantity":null})).unwrap();
        assert_eq!(category.inventory_kind, InventoryKind::Unknown);
        assert_eq!(
            serde_json::to_value(&offer).unwrap()["inventory_kind"],
            "unknown"
        );
        assert_eq!(
            serde_json::to_value(InventoryKind::Primary).unwrap(),
            "primary"
        );
        assert!(
            serde_json::from_value::<Offer>(
                serde_json::json!({"key":"bad","inventory_kind":"mixed"})
            )
            .is_err()
        );
    }
}
