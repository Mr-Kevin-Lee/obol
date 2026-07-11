//! A three-color status band, shared across every threshold-based
//! recommendation metric (spec §13.1 Types A/B, D39). Extracted out of
//! `emergency_fund.rs` (its original, sole home) once `monthly_spend.rs`
//! became a second real consumer with an *inverted* comparison
//! direction ("higher is worse" vs. emergency fund's "lower is worse")
//! — the enum itself has no domain-specific semantics (just three
//! labels), so it doesn't belong to either module specifically. Each
//! domain module still owns its own `band_for_*` comparison function.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdBand {
    Red,
    Yellow,
    Green,
}

impl ThresholdBand {
    pub fn label(&self) -> &'static str {
        match self {
            ThresholdBand::Red => "Red",
            ThresholdBand::Yellow => "Yellow",
            ThresholdBand::Green => "Green",
        }
    }
}
