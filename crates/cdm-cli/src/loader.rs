//! Turning CLI arguments into a loaded configuration (`CFG-010`, `CLI-002`).
//!
//! The precedence order lives in `cdm-config`; this module only feeds it. Java's spellings
//! (`--properties-file`, `--conf`) and the canonical ones (`--config`, `--set`) are both accepted
//! and can be mixed, which is what makes an incremental migration off Java possible: an operator
//! can keep their existing properties file and override one key at a time.

use cdm_config::{ConfigLoader, LoadOutcome};
use cdm_core::CdmError;

use crate::cli::ConfigArgs;

/// Builds and runs the configuration loader for a command's arguments.
pub fn load(args: &ConfigArgs) -> Result<LoadOutcome, CdmError> {
    let mut loader = ConfigLoader::new();

    if let Some(path) = &args.config {
        loader = loader.with_file(path);
    }
    if let Some(path) = &args.properties_file {
        loader = loader.with_file(path);
    }

    loader = loader.with_process_env();

    // `--conf` carries Java names and `--set` carries canonical ones, but the loader resolves
    // aliases itself, so both go through the same door. `--set` is applied second so it wins,
    // matching the documented precedence.
    if !args.conf.is_empty() {
        loader = loader.with_overrides(args.conf.iter().map(String::as_str));
    }
    if !args.set.is_empty() {
        loader = loader.with_overrides(args.set.iter().map(String::as_str));
    }
    if let Some(profile) = &args.profile {
        loader = loader.with_profile(profile);
    }

    Ok(loader.load())
}
