//! Integration test entrypoint for CLI smoke suites.

mod support;

#[path = "cli_smoke/collections.rs"]
mod collections;
#[path = "cli_smoke/global_options.rs"]
mod global_options;
#[path = "cli_smoke/help.rs"]
mod help;
#[path = "cli_smoke/lockfile.rs"]
mod lockfile;
#[path = "cli_smoke/read_configuration.rs"]
mod read_configuration;
#[path = "cli_smoke/version.rs"]
mod version;
