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
        Err(error) if missing_config_is_optional_for_container_inspection(args, &error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn missing_config_is_optional_for_container_inspection(args: &[String], error: &str) -> bool {
    common::parse_option_value(args, "--container-id").is_some()
        && common::config_option_value(args).is_none()
        && common::parse_option_value(args, "--workspace-folder").is_none()
        && error.starts_with("Unable to locate a dev container config at ")
}

#[cfg(test)]
mod tests {
    use super::missing_config_is_optional_for_container_inspection;
    use crate::commands::common::{test_env_defaults, DEVCONTAINER_CONFIG};

    #[test]
    fn missing_config_is_optional_only_for_container_inspection_without_explicit_sources() {
        let missing_error =
            "Unable to locate a dev container config at /workspace/.devcontainer/devcontainer.json";

        assert!(missing_config_is_optional_for_container_inspection(
            &args(&["--container-id", "container-123"]),
            missing_error
        ));
        assert!(
            !missing_config_is_optional_for_container_inspection(
                &args(&[
                    "--container-id",
                    "container-123",
                    "--workspace-folder",
                    "/workspace",
                ]),
                missing_error
            ),
            "explicit workspace must not ignore missing config"
        );
        assert!(
            !missing_config_is_optional_for_container_inspection(
                &args(&[
                    "--container-id",
                    "container-123",
                    "--config",
                    "devcontainer.json",
                ]),
                missing_error
            ),
            "explicit config must not ignore missing config"
        );
        let env = test_env_defaults(&[(DEVCONTAINER_CONFIG, "missing-devcontainer.json")]);
        assert!(
            !missing_config_is_optional_for_container_inspection(
                &args(&["--container-id", "container-123"]),
                missing_error
            ),
            "environment config must not be ignored"
        );
        drop(env);
        assert!(
            !missing_config_is_optional_for_container_inspection(&args(&[]), missing_error),
            "missing config without container inspection must remain an error"
        );
        assert!(
            !missing_config_is_optional_for_container_inspection(
                &args(&["--container-id", "container-123"]),
                "permission denied"
            ),
            "non-discovery errors must remain errors"
        );
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }
}
