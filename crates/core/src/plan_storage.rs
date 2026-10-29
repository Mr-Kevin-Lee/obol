//! `plan.yaml` (spec §13.6, D44): category targets, confirmed series and
//! dismissed suggestions.
//!
//! **Contract (task 111b), implemented by task 115.** A separate file from
//! `rules.yaml` for the same reason as `merchant_rules.yaml`. Writes go
//! through `atomic_write`.

use std::path::Path;

use thiserror::Error;

use crate::spending_plan::PlanConfig;

#[derive(Debug, Error)]
pub enum PlanStorageError {
    #[error("plan.yaml could not be parsed: {0}")]
    Parse(serde_saphyr::Error),
    #[error("failed to write plan.yaml: {0}")]
    Serialize(serde_saphyr::ser_error::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Loads `plan.yaml`, creating an empty one if it doesn't exist.
#[allow(unused_variables)] // stub until task 115
pub fn load_or_init_plan(path: &Path) -> Result<PlanConfig, PlanStorageError> {
    todo!("task 115")
}

#[allow(unused_variables)] // stub until task 115
pub fn save_plan(path: &Path, plan: &PlanConfig) -> Result<(), PlanStorageError> {
    todo!("task 115")
}
