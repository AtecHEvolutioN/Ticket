use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Unavailable,
    Unknown,
    Blocked,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    Metadata,
    ExplicitStatus,
    Offers,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuantityKind {
    #[default]
    Unknown,
    Listing,
    Selectable,
    Remaining,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PriceScope {
    #[default]
    Unknown,
    PerTicket,
    Total,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FeeScope {
    #[default]
    Unknown,
    Included,
    Excluded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Offer {
    pub key: String,
    pub section: Option<String>,
    pub row: Option<String>,
    pub seats: Option<String>,
    pub ticket_type: Option<String>,
    pub inventory: Option<String>,
    pub price_text: Option<String>,
    pub price_minor: Option<i64>,
    pub currency: Option<String>,
    pub quantity: Option<u32>,
    #[serde(default)]
    pub quantity_kind: QuantityKind,
    #[serde(default)]
    pub price_scope: PriceScope,
    #[serde(default)]
    pub fees: FeeScope,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TicketCategory {
    pub key: String,
    pub ticket_type: String,
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub row: Option<String>,
    pub price_text: Option<String>,
    pub price_min_minor: Option<i64>,
    pub price_max_minor: Option<i64>,
    pub currency: Option<String>,
    /// The purchase control's capacity, never remaining inventory.
    pub selectable_quantity: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub watch_id: String,
    pub observed_at: i64,
    pub availability: Availability,
    pub evidence: Evidence,
    pub complete: bool,
    pub offers: Vec<Offer>,
    #[serde(default)]
    pub ticket_categories: Vec<TicketCategory>,
    #[serde(default)]
    pub adapter: Option<String>,
    pub detail: String,
}

impl Observation {
    pub fn unknown(id: &str, at: i64, detail: impl Into<String>) -> Self {
        Self {
            watch_id: id.into(),
            observed_at: at,
            availability: Availability::Unknown,
            evidence: Evidence::None,
            complete: false,
            offers: vec![],
            ticket_categories: vec![],
            adapter: None,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub watch_id: String,
    pub event_name: String,
    pub artist: String,
    pub location: String,
    pub event_url: String,
    pub observed_at: i64,
    pub kind: String,
    pub detail: String,
    pub offers: Vec<Offer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAlert {
    pub id: i64,
    pub nonce: String,
    pub payload: Alert,
    pub attempts: u32,
}
