//! Configuration inspection helpers for runtime and read-configuration flows.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Map, Value};

use super::merge::merge_configuration;
use super::{InspectedContainer, LoadedConfig};
use crate::config::{self, ConfigContext};
use crate::runtime;

pub(super) fn read_configuration_value(
    loaded: Option<&LoadedConfig>,
    inspected: Option<&InspectedContainer>,
) -> Value {
    let mut configuration = loaded
        .map(|value| {
            let mut configuration = value.configuration.clone();
            if let Value::Object(entries) = &mut configuration {
                entries.insert(
                    "configFilePath".to_string(),
                    Value::String(value.config_file.display().to_string()),
                );
            }
            configuration
        })
        .unwrap_or(Value::Object(Map::new()));

    if let Some(inspected) = inspected {
        configuration = config::substitute_container_env(&configuration, &inspected.container_env);
    }

    configuration
}

pub(super) fn workspace_payload(
    loaded: &LoadedConfig,
    configuration: &Value,
    args: &[String],
) -> Value {
    let resolved = runtime::context::ResolvedConfig {
        workspace_folder: loaded.workspace_folder.clone(),
        config_file: loaded.config_file.clone(),
        configuration: configuration.clone(),
    };
    let workspace_folder = runtime::context::remote_workspace_folder_for_args(&resolved, args);
    let mut payload = Map::new();
    payload.insert(
        "workspaceFolder".to_string(),
        Value::String(workspace_folder.clone()),
    );
    if !runtime::compose::uses_compose_config(configuration) {
        payload.insert(
            "workspaceMount".to_string(),
            Value::String(runtime::context::workspace_mount_for_args(
                &resolved,
                &workspace_folder,
                args,
            )),
        );
    }
    Value::Object(payload)
}

pub(super) fn inspect_container(
    args: &[String],
    container_id: &str,
    loaded: Option<&LoadedConfig>,
) -> Result<InspectedContainer, String> {
    let result =
        runtime::engine::run_engine(args, vec!["inspect".to_string(), container_id.to_string()])?;
    if result.status_code != 0 {
        return Err(runtime::engine::stderr_or_stdout(&result));
    }

    let inspected: Value = serde_json::from_str(&result.stdout)
        .map_err(|error| format!("Invalid inspect JSON: {error}"))?;
    let details = inspected
        .as_array()
        .and_then(|entries| entries.first())
        .ok_or("Container engine did not return inspect details".to_string())?;
    let labels = details
        .get("Config")
        .and_then(|value| value.get("Labels"))
        .and_then(Value::as_object);
    let container_env = details
        .get("Config")
        .and_then(|value| value.get("Env"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|entry| {
                    entry
                        .split_once('=')
                        .map(|(name, value)| (name.to_string(), value.to_string()))
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let local_workspace_folder = loaded
        .map(|value| value.workspace_folder.clone())
        .or_else(|| {
            labels
                .and_then(|entries| entries.get("devcontainer.local_folder"))
                .and_then(Value::as_str)
                .map(PathBuf::from)
        });
    let mut metadata_entries = runtime::metadata::metadata_entries(
        labels
            .and_then(|entries| entries.get("devcontainer.metadata"))
            .and_then(Value::as_str),
    );
    if let Some(workspace_folder) = local_workspace_folder {
        let context = ConfigContext {
            workspace_folder,
            env: std::env::vars().collect(),
            container_workspace_folder: None,
            id_labels: HashMap::new(),
        };
        metadata_entries = metadata_entries
            .into_iter()
            .map(|entry| config::substitute_local_context(&entry, &context))
            .collect();
    }
    metadata_entries = metadata_entries
        .into_iter()
        .map(|entry| config::substitute_container_env(&entry, &container_env))
        .collect();

    Ok(InspectedContainer {
        metadata_entries,
        container_env,
    })
}

pub(super) fn merged_configuration_payload(
    configuration: &Value,
    inspected: Option<&InspectedContainer>,
    additional_metadata_entries: &[Value],
) -> Value {
    let mut metadata_entries = inspected
        .map(|value| value.metadata_entries.clone())
        .unwrap_or_default();
    metadata_entries.extend(additional_metadata_entries.iter().cloned());
    let config_metadata = pick_config_metadata(configuration);
    if config_metadata
        .as_object()
        .is_some_and(|entries| !entries.is_empty())
    {
        metadata_entries.push(config_metadata);
    }
    merge_configuration(configuration, &metadata_entries)
}

fn pick_config_metadata(configuration: &Value) -> Value {
    let Some(entries) = configuration.as_object() else {
        return Value::Object(Map::new());
    };
    let mut picked = Map::new();
    for key in [
        "onCreateCommand",
        "updateContentCommand",
        "postCreateCommand",
        "postStartCommand",
        "postAttachCommand",
        "waitFor",
        "customizations",
        "mounts",
        "containerEnv",
        "containerUser",
        "init",
        "privileged",
        "capAdd",
        "securityOpt",
        "remoteUser",
        "userEnvProbe",
        "remoteEnv",
        "overrideCommand",
        "portsAttributes",
        "otherPortsAttributes",
        "forwardPorts",
        "shutdownAction",
        "updateRemoteUserUID",
        "hostRequirements",
        "entrypoint",
    ] {
        if let Some(value) = entries.get(key) {
            picked.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(picked)
}
