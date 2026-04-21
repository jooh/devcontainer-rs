//! Compose override-file generation for native runtime flows.

#[path = "override_mounts.rs"]
mod override_mounts;
#[path = "override_yaml.rs"]
mod override_yaml;

use std::path::PathBuf;

use serde_json::Value;

use crate::commands::common;

use super::super::container;
use super::super::context::ResolvedConfig;
use super::super::metadata::serialized_container_metadata;
use super::super::paths::unique_temp_path;
use super::service::{self, ServiceDefinition};
use super::ComposeSpec;
use override_mounts::{
    compose_additional_volumes, compose_environment, compose_named_volumes,
    compose_workspace_volume,
};
use override_yaml::{
    escape_compose_label, escape_compose_scalar, render_compose_string_sequence,
    render_compose_volume_entry, render_named_volume_entry,
};

pub(super) fn compose_build_override_file(
    spec: &ComposeSpec,
    args: &[String],
) -> Result<Option<PathBuf>, String> {
    let cache_from = common::parse_option_values(args, "--cache-from");
    if cache_from.is_empty() {
        return Ok(None);
    }

    let mut content = service::read_version_prefix(&spec.files).unwrap_or_default();
    content.push_str(&format!(
        "services:\n  '{}':\n    build:\n      cache_from:\n",
        spec.service
    ));
    for value in cache_from {
        content.push_str(&format!("        - '{}'\n", escape_compose_scalar(&value)));
    }

    let override_file = unique_temp_path("devcontainer-compose-build-override", Some("yml"));
    std::fs::write(&override_file, content).map_err(|error| error.to_string())?;
    Ok(Some(override_file))
}

pub(super) fn compose_metadata_override_file(
    resolved: &ResolvedConfig,
    args: &[String],
    remote_workspace_folder: &str,
    image_name: Option<&str>,
) -> Result<Option<PathBuf>, String> {
    let service_name = resolved
        .configuration
        .get("service")
        .and_then(Value::as_str)
        .ok_or_else(|| "Compose configuration must define service".to_string())?;
    let metadata = serialized_container_metadata(
        &resolved.configuration,
        remote_workspace_folder,
        common::runtime_options(args).omit_config_remote_env_from_metadata,
    )?;
    let mut labels =
        common::default_devcontainer_id_labels(&resolved.workspace_folder, &resolved.config_file);
    labels.push(format!("devcontainer.metadata={metadata}"));
    labels.extend(common::parse_option_values(args, "--id-label"));
    if labels.is_empty() {
        return Ok(None);
    }

    let (version_prefix, service_definition) = compose_override_context(resolved, service_name);
    let mut content = version_prefix;
    content.push_str("services:\n");
    content.push_str(&format!(
        "  '{}':\n    labels:{}\n",
        service_name,
        labels
            .iter()
            .map(|label| format!("\n      - '{}'", escape_compose_label(label)))
            .collect::<String>()
    ));
    if let Some(image_name) = image_name {
        content.push_str(&format!(
            "    image: '{}'\n",
            escape_compose_scalar(image_name)
        ));
    }
    content.push_str(&format!(
        "    entrypoint: {}\n",
        compose_wrapper_entrypoint(resolved, service_definition.as_ref())?
    ));
    if let Some(command) = compose_wrapper_command(resolved, service_definition.as_ref())? {
        content.push_str(&format!("    command: {command}\n"));
    }
    let mut volumes = Vec::new();
    if let Some(volume) = compose_workspace_volume(resolved, args, remote_workspace_folder) {
        volumes.push(volume);
    }
    volumes.extend(compose_additional_volumes(resolved, args)?);
    let named_volumes = compose_named_volumes(&volumes);
    if !volumes.is_empty() {
        content.push_str("\n    volumes:\n");
        for volume in &volumes {
            content.push_str(&render_compose_volume_entry(volume));
        }
    }
    if let Some(environment) = compose_environment(&resolved.configuration) {
        content.push_str("    environment:\n");
        for (key, value) in environment {
            content.push_str(&format!(
                "      {}: '{}'\n",
                key,
                escape_compose_scalar(&value)
            ));
        }
    }
    if let Some(user) = resolved
        .configuration
        .get("containerUser")
        .and_then(Value::as_str)
    {
        content.push_str(&format!("    user: '{}'\n", escape_compose_scalar(user)));
    }
    if resolved
        .configuration
        .get("privileged")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        content.push_str("    privileged: true\n");
    }
    if resolved
        .configuration
        .get("init")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        content.push_str("    init: true\n");
    }
    if let Some(cap_add) = resolved
        .configuration
        .get("capAdd")
        .and_then(Value::as_array)
    {
        content.push_str("    cap_add:\n");
        for capability in cap_add.iter().filter_map(Value::as_str) {
            content.push_str(&format!(
                "      - '{}'\n",
                escape_compose_scalar(capability)
            ));
        }
    }
    if let Some(security_opt) = resolved
        .configuration
        .get("securityOpt")
        .and_then(Value::as_array)
    {
        content.push_str("    security_opt:\n");
        for option in security_opt.iter().filter_map(Value::as_str) {
            content.push_str(&format!("      - '{}'\n", escape_compose_scalar(option)));
        }
    }
    if container::should_add_gpu_capability(&resolved.configuration, args)? {
        content.push_str(
            "    deploy:\n      resources:\n        reservations:\n          devices:\n            - capabilities: [gpu]\n",
        );
    }
    if !named_volumes.is_empty() {
        content.push_str("\nvolumes:\n");
        for named_volume in &named_volumes {
            content.push_str(&render_named_volume_entry(named_volume));
        }
    }

    let override_file = unique_override_file_path();
    std::fs::write(&override_file, content).map_err(|error| error.to_string())?;
    Ok(Some(override_file))
}

fn compose_override_context(
    resolved: &ResolvedConfig,
    service_name: &str,
) -> (String, Option<ServiceDefinition>) {
    let config_root = resolved
        .config_file
        .parent()
        .unwrap_or(resolved.workspace_folder.as_path());
    let Ok(compose_files) = service::compose_files(
        &resolved.configuration,
        config_root,
        &resolved.workspace_folder,
    ) else {
        return (String::new(), None);
    };
    let version_prefix = service::read_version_prefix(&compose_files).unwrap_or_default();
    let service_definition = service::inspect_service_definition(&compose_files, service_name).ok();
    (version_prefix, service_definition)
}

fn compose_wrapper_entrypoint(
    resolved: &ResolvedConfig,
    service_definition: Option<&ServiceDefinition>,
) -> Result<String, String> {
    let override_command = resolved
        .configuration
        .get("overrideCommand")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let compose_entrypoint =
        service_definition.and_then(|definition| definition.entrypoint.clone());
    let user_entrypoint = if override_command {
        Vec::new()
    } else {
        compose_entrypoint.unwrap_or_default()
    };
    let custom_entrypoints = merged_entrypoints(resolved).join("\n\n");
    let script = format!(
        "echo Container started\ntrap \"exit 0\" 15\n{custom_entrypoints}\nexec \"$$@\"\nwhile sleep 1 & wait $$!; do :; done"
    );
    let mut entrypoint = vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        script,
        "-".to_string(),
    ];
    entrypoint.extend(user_entrypoint);
    render_compose_string_sequence(&entrypoint)
}

fn compose_wrapper_command(
    resolved: &ResolvedConfig,
    service_definition: Option<&ServiceDefinition>,
) -> Result<Option<String>, String> {
    let override_command = resolved
        .configuration
        .get("overrideCommand")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let compose_entrypoint =
        service_definition.and_then(|definition| definition.entrypoint.clone());
    let compose_command = service_definition.and_then(|definition| definition.command.clone());
    let user_command = if override_command {
        Some(Vec::new())
    } else if let Some(command) = compose_command.clone() {
        Some(command)
    } else if compose_entrypoint.is_some() {
        Some(Vec::new())
    } else {
        None
    };
    if user_command == compose_command {
        return Ok(None);
    }
    user_command
        .map(|command| render_compose_string_sequence(&command))
        .transpose()
}

fn merged_entrypoints(resolved: &ResolvedConfig) -> Vec<String> {
    if let Some(values) = resolved
        .configuration
        .get("entrypoints")
        .and_then(Value::as_array)
    {
        return values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    resolved
        .configuration
        .get("entrypoint")
        .and_then(Value::as_str)
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

fn unique_override_file_path() -> PathBuf {
    unique_temp_path("devcontainer-compose-override", Some("yml"))
}
