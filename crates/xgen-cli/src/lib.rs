#![doc = "Composition primitives for the local-first `XGEN` CLI."]

mod allow_file;
mod allow_path;
mod allow_process;
mod composition;
mod driver;
mod environment;
mod final_response;
mod graph_discovery;
mod http_read;
mod manifest;
mod material_catalog;
mod model_profile;
mod run_layout;
#[doc(hidden)]
pub mod tools;
mod usage;

pub use composition::*;
pub use driver::*;
pub use manifest::MAX_HOST_MODEL_TURNS;
pub use model_profile::*;
pub use usage::{CostEstimate, TokenPrices, UsageReport, UsdPerMillion, inspect_local_usage};

#[doc(hidden)]
pub use environment::compatible_environment;

/// Resolve the catalog directory using the same platform and legacy state roots as Runs.
#[doc(hidden)]
pub fn tool_catalog_directory() -> Result<std::path::PathBuf, &'static str> {
    run_layout::discover_state_root()
        .map(|root| root.join("tool-collections"))
        .map_err(|_| "invalid_state_home")
}
