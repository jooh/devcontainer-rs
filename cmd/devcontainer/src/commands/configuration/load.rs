//! Configuration loading helpers for command entrypoints.

use std::fs;

use super::LoadedConfig;
use crate::commands::common;

pub(super) fn load_config(args: &[String]) -> Result<LoadedConfig, String> {
    let (workspace_folder, config_file) = common::resolve_read_configuration_path(args)?;
    let config_source = common::resolve_override_config_path(args)?.unwrap_or(config_file.clone());
    let raw_text = fs::read_to_string(&config_source).map_err(|error| error.to_string())?;
    let configuration = common::load_resolved_config(args)?.2;
    Ok(LoadedConfig {
        workspace_folder,
        config_file,
        raw_text,
        configuration,
    })
}

pub(super) fn load_optional_config(args: &[String]) -> Result<Option<LoadedConfig>, String> {
    match load_config(args) {
        Ok(loaded) => Ok(Some(loaded)),
        Err(error)
            if common::parse_option_value(args, "--container-id").is_some()
                && common::parse_option_value(args, "--config").is_none()
                && common::parse_option_value(args, "--workspace-folder").is_none()
                && error.starts_with("Unable to locate a dev container config at ") =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}
