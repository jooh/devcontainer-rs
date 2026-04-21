//! Container inspection helpers for deriving runtime context from existing containers.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::commands::common;
use crate::config::{self, ConfigContext};

use super::super::engine;
use super::configured_user;

pub(super) struct InspectedContainerContext {
    pub(super) configuration: Value,
    pub(super) local_workspace_folder: Option<PathBuf>,
    pub(super) remote_workspace_folder: Option<String>,
}

pub(super) fn inspect_container_context(
    args: &[String],
    container_id: &str,
) -> Result<InspectedContainerContext, String> {
    let result = engine::run_engine(args, vec!["inspect".to_string(), container_id.to_string()])?;
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }

    let inspected: Value = serde_json::from_str(&result.stdout)
        .map_err(|error| format!("Invalid inspect JSON: {error}"))?;
    let details = inspected
        .as_array()
        .and_then(|entries| entries.first())
        .ok_or_else(|| "Container engine did not return inspect details".to_string())?;
    let labels = details
        .get("Config")
        .and_then(|value| value.get("Labels"))
        .and_then(Value::as_object);
    let local_workspace_folder = labels
        .and_then(|entries| entries.get("devcontainer.local_folder"))
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let mut configuration = crate::runtime::metadata::merged_container_metadata(
        labels
            .and_then(|entries| entries.get("devcontainer.metadata"))
            .and_then(Value::as_str),
    );
    if let Some(workspace_folder) = &local_workspace_folder {
        configuration = config::substitute_local_context(
            &configuration,
            &ConfigContext {
                workspace_folder: workspace_folder.clone(),
                env: env::vars().collect(),
                container_workspace_folder: None,
                id_labels: HashMap::new(),
            },
        );
    }
    if configured_user(&configuration).is_none() {
        if let Some(user) = details
            .get("Config")
            .and_then(|value| value.get("User"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            if let Value::Object(entries) = &mut configuration {
                entries.insert("containerUser".to_string(), Value::String(user.to_string()));
            }
        }
    }

    Ok(InspectedContainerContext {
        remote_workspace_folder: configuration
            .get("workspaceFolder")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| inspect_workspace_mount(details, local_workspace_folder.as_deref())),
        configuration,
        local_workspace_folder,
    })
}

fn inspect_workspace_mount(
    details: &Value,
    local_workspace_folder: Option<&Path>,
) -> Option<String> {
    let mounts = details.get("Mounts").and_then(Value::as_array)?;
    if let Some(local_workspace_folder) = local_workspace_folder {
        let local_workspace_folder = common::normalize_devcontainer_label_path(
            &local_workspace_folder.display().to_string(),
        );
        if let Some(destination) = mounts.iter().find_map(|mount| {
            let source = mount
                .get("Source")
                .and_then(Value::as_str)
                .map(common::normalize_devcontainer_label_path);
            (source.as_deref() == Some(local_workspace_folder.as_str()))
                .then(|| mount.get("Destination").and_then(Value::as_str))
                .flatten()
        }) {
            return Some(destination.to_string());
        }
    }
    mounts
        .iter()
        .find_map(|mount| mount.get("Destination").and_then(Value::as_str))
        .map(str::to_string)
}

pub(super) fn workspace_folder_from_args(args: &[String]) -> Result<Option<PathBuf>, String> {
    if let Some(workspace_folder) = common::parse_option_value(args, "--workspace-folder") {
        return Ok(Some(
            fs::canonicalize(&workspace_folder).unwrap_or_else(|_| PathBuf::from(workspace_folder)),
        ));
    }
    match env::current_dir() {
        Ok(path) => Ok(Some(path)),
        Err(error) => Err(error.to_string()),
    }
}
