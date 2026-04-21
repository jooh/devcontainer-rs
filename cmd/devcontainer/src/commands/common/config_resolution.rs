//! Workspace and config resolution helpers shared across commands.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::{self, ConfigContext};
use crate::runtime::mounts::mount_option_target;

use super::args::{parse_option_value, validate_option_values};
use super::labels::id_label_map;

pub(crate) fn resolve_read_configuration_path(
    args: &[String],
) -> Result<(PathBuf, PathBuf), String> {
    validate_option_values(
        args,
        &["--workspace-folder", "--config", "--override-config"],
    )?;

    let explicit_workspace = parse_option_value(args, "--workspace-folder").map(PathBuf::from);
    let explicit_config = parse_option_value(args, "--config").map(PathBuf::from);
    let override_config = resolve_override_config_path(args)?;

    let initial_workspace = explicit_workspace
        .clone()
        .or_else(|| env::current_dir().ok())
        .ok_or_else(|| "Unable to determine workspace folder".to_string())?;

    let workspace_folder = if explicit_workspace.is_some() {
        initial_workspace.clone()
    } else if let Some(explicit_config) = explicit_config.as_deref() {
        let config_path = config::expected_config_path(&initial_workspace, Some(explicit_config));
        infer_workspace_folder_from_config(&config_path)
    } else if let Some(override_config) = override_config.as_deref() {
        infer_workspace_folder_from_config(override_config)
    } else {
        initial_workspace.clone()
    };

    let config_path = if override_config.is_some() {
        let expected = config::expected_config_path(&workspace_folder, explicit_config.as_deref());
        fs::canonicalize(&expected).unwrap_or(expected)
    } else {
        config::resolve_config_path(&workspace_folder, explicit_config.as_deref())?
    };

    let resolved_workspace = if explicit_workspace.is_some() {
        fs::canonicalize(&workspace_folder).unwrap_or(workspace_folder)
    } else if explicit_config.is_some() {
        infer_workspace_folder_from_config(&config_path)
    } else if override_config.is_some() {
        infer_workspace_folder_from_config(override_config.as_deref().expect("override config"))
    } else {
        fs::canonicalize(&initial_workspace).unwrap_or(initial_workspace)
    };
    Ok((resolved_workspace, config_path))
}

fn infer_workspace_folder_from_config(config_path: &Path) -> PathBuf {
    let config_parent = config_path.parent().unwrap_or(config_path);
    let workspace = config_path
        .ancestors()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(".devcontainer"))
        .and_then(Path::parent)
        .unwrap_or(config_parent);
    fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf())
}

pub(crate) fn load_resolved_config(args: &[String]) -> Result<(PathBuf, PathBuf, Value), String> {
    load_resolved_config_with_label_override(args, None)
}

pub(crate) fn load_resolved_config_with_id_labels(
    args: &[String],
    id_labels: HashMap<String, String>,
) -> Result<(PathBuf, PathBuf, Value), String> {
    load_resolved_config_with_label_override(args, Some(id_labels))
}

fn load_resolved_config_with_label_override(
    args: &[String],
    id_labels: Option<HashMap<String, String>>,
) -> Result<(PathBuf, PathBuf, Value), String> {
    let (workspace_folder, config_file) = resolve_read_configuration_path(args)?;
    let config_source = resolve_override_config_path(args)?.unwrap_or_else(|| config_file.clone());
    let raw = fs::read_to_string(&config_source).map_err(|error| error.to_string())?;
    let parsed = config::parse_jsonc_value(&raw)?;
    let id_labels =
        id_labels.unwrap_or_else(|| id_label_map(args, &workspace_folder, &config_file));
    let base_context = ConfigContext {
        workspace_folder: workspace_folder.clone(),
        env: env::vars().collect(),
        container_workspace_folder: None,
        id_labels: id_labels.clone(),
    };
    let container_workspace_folder = parsed
        .get("workspaceFolder")
        .and_then(Value::as_str)
        .map(|value| {
            config::substitute_local_context(&Value::String(value.to_string()), &base_context)
        })
        .and_then(|value| value.as_str().map(str::to_string))
        .or_else(|| {
            parsed
                .get("workspaceMount")
                .and_then(Value::as_str)
                .and_then(|mount| {
                    let substituted = config::substitute_local_context(
                        &Value::String(mount.to_string()),
                        &base_context,
                    );
                    substituted.as_str().and_then(mount_option_target)
                })
        })
        .or_else(|| {
            Some(
                if crate::runtime::compose::uses_compose_config(&parsed)
                    && parsed.get("workspaceFolder").is_none()
                    && parsed.get("workspaceMount").is_none()
                {
                    "/".to_string()
                } else {
                    crate::runtime::context::derived_workspace_mount(&workspace_folder, args)
                        .map(|derived| derived.remote_workspace_folder)
                        .unwrap_or_else(|| {
                            crate::runtime::context::default_remote_workspace_folder(Some(
                                &workspace_folder,
                            ))
                        })
                },
            )
        });
    let substituted = config::substitute_local_context(
        &parsed,
        &ConfigContext {
            workspace_folder: base_context.workspace_folder.clone(),
            env: base_context.env,
            container_workspace_folder,
            id_labels,
        },
    );
    Ok((workspace_folder, config_file, substituted))
}

pub(crate) fn resolve_override_config_path(args: &[String]) -> Result<Option<PathBuf>, String> {
    let Some(path) = parse_option_value(args, "--override-config") else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    let resolved = if path.is_absolute() {
        path
    } else {
        env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    if !resolved.is_file() {
        return Err(format!(
            "Unable to locate an override dev container config at {}",
            resolved.display()
        ));
    }
    Ok(Some(fs::canonicalize(&resolved).unwrap_or(resolved)))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;

    use crate::commands::common::DEVCONTAINER_LOCAL_FOLDER_LABEL;
    use crate::test_support::unique_temp_dir;

    use super::{load_resolved_config, load_resolved_config_with_id_labels};

    #[test]
    fn load_resolved_config_with_id_labels_recomputes_devcontainer_id_from_override_labels() {
        let workspace = unique_temp_dir("devcontainer-config-resolution");
        let config_dir = workspace.join(".devcontainer");
        let config_file = config_dir.join("devcontainer.json");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::write(
            &config_file,
            "{\n  \"mounts\": [{\n    \"source\": \"cache-${devcontainerId}\",\n    \"target\": \"/cache\",\n    \"type\": \"volume\"\n  }],\n  \"postAttachCommand\": \"echo ${devcontainerId}\"\n}\n",
        )
        .expect("config write");

        let args = vec![
            "--workspace-folder".to_string(),
            workspace.display().to_string(),
        ];
        let (_, _, current) = load_resolved_config(&args).expect("current config");
        let (_, _, legacy) = load_resolved_config_with_id_labels(
            &args,
            HashMap::from([(
                DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
                workspace.display().to_string(),
            )]),
        )
        .expect("legacy config");

        assert_ne!(
            current["mounts"][0]["source"],
            legacy["mounts"][0]["source"]
        );
        assert_ne!(current["postAttachCommand"], legacy["postAttachCommand"]);

        let _ = fs::remove_dir_all(workspace);
    }
}
