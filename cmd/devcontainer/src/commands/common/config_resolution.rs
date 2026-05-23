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

    let initial_workspace = match explicit_workspace.clone() {
        Some(path) => path,
        None => {
            env::current_dir().map_err(|_| "Unable to determine workspace folder".to_string())?
        }
    };

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

    let resolved_workspace = resolved_workspace_path(
        explicit_workspace.is_some(),
        explicit_config.is_some(),
        override_config.as_deref(),
        workspace_folder,
        &config_path,
        initial_workspace,
    );
    Ok((resolved_workspace, config_path))
}

fn resolved_workspace_path(
    has_explicit_workspace: bool,
    has_explicit_config: bool,
    override_config: Option<&Path>,
    workspace_folder: PathBuf,
    config_path: &Path,
    initial_workspace: PathBuf,
) -> PathBuf {
    if has_explicit_workspace {
        fs::canonicalize(&workspace_folder).unwrap_or(workspace_folder)
    } else if has_explicit_config {
        infer_workspace_folder_from_config(config_path)
    } else if let Some(override_config) = override_config {
        infer_workspace_folder_from_config(override_config)
    } else {
        fs::canonicalize(&initial_workspace).unwrap_or(initial_workspace)
    }
}

fn infer_workspace_folder_from_config(config_path: &Path) -> PathBuf {
    let config_parent = config_path.parent().unwrap_or(config_path);
    let workspace = config_path
        .ancestors()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(".devcontainer"))
        .and_then(Path::parent)
        .unwrap_or(config_parent);
    match fs::canonicalize(workspace) {
        Ok(path) => path,
        Err(_) => workspace.to_path_buf(),
    }
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
    let config_source = resolve_override_config_path(args)?.unwrap_or(config_file.clone());
    let raw = fs::read_to_string(&config_source).map_err(|error| error.to_string())?;
    let parsed = config::parse_jsonc_value(&raw)?;
    let id_labels = match id_labels {
        Some(id_labels) => id_labels,
        None => id_label_map(args, &workspace_folder, &config_file),
    };
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
                        .expect("workspace mount derivation should always return a default")
                        .remote_workspace_folder
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

    use super::{
        load_resolved_config, load_resolved_config_with_id_labels, resolve_override_config_path,
        resolved_workspace_path,
    };

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

    #[test]
    fn resolved_workspace_path_handles_explicit_config_override_and_default_inputs() {
        let root = unique_temp_dir("devcontainer-resolved-workspace-path");
        let explicit_workspace = root.join("explicit-workspace");
        let config_workspace = root.join("config-workspace");
        let config_dir = config_workspace.join(".devcontainer");
        let config_file = config_dir.join("devcontainer.json");
        let override_workspace = root.join("override-workspace");
        let override_dir = override_workspace.join(".devcontainer");
        let override_file = override_dir.join("devcontainer.json");
        let default_workspace = root.join("default-workspace");
        fs::create_dir_all(&explicit_workspace).expect("explicit workspace");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::create_dir_all(&override_dir).expect("override dir");
        fs::create_dir_all(&default_workspace).expect("default workspace");

        assert_eq!(
            resolved_workspace_path(
                true,
                false,
                None,
                explicit_workspace.clone(),
                &config_file,
                default_workspace.clone(),
            ),
            fs::canonicalize(&explicit_workspace).expect("canonical explicit workspace")
        );
        assert_eq!(
            resolved_workspace_path(
                false,
                true,
                None,
                explicit_workspace,
                &config_file,
                default_workspace.clone(),
            ),
            fs::canonicalize(&config_workspace).expect("canonical config workspace")
        );
        assert_eq!(
            resolved_workspace_path(
                false,
                false,
                Some(&override_file),
                config_workspace,
                &config_file,
                default_workspace.clone(),
            ),
            fs::canonicalize(&override_workspace).expect("canonical override workspace")
        );
        assert_eq!(
            resolved_workspace_path(
                false,
                false,
                None,
                override_workspace,
                &config_file,
                default_workspace.clone(),
            ),
            fs::canonicalize(&default_workspace).expect("canonical default workspace")
        );
        assert_eq!(
            resolved_workspace_path(
                false,
                true,
                None,
                root.join("unused"),
                &root.join("missing-workspace/.devcontainer/devcontainer.json"),
                default_workspace,
            ),
            root.join("missing-workspace")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_resolved_config_substitutes_workspace_folder_setting() {
        let workspace = unique_temp_dir("devcontainer-config-resolution-workspace-folder");
        let config_dir = workspace.join(".devcontainer");
        let config_file = config_dir.join("devcontainer.json");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::write(
            &config_file,
            r#"{
                "workspaceFolder": "${localWorkspaceFolder}/inside",
                "remoteEnv": {
                    "HERE": "${containerWorkspaceFolder}"
                }
            }"#,
        )
        .expect("config write");

        let (_, _, config) = load_resolved_config(&[
            "--workspace-folder".to_string(),
            workspace.display().to_string(),
        ])
        .expect("config");
        let canonical_workspace = fs::canonicalize(&workspace).expect("canonical workspace");

        assert_eq!(
            config["workspaceFolder"],
            format!("{}/inside", canonical_workspace.display())
        );
        assert_eq!(
            config["remoteEnv"]["HERE"],
            format!("{}/inside", canonical_workspace.display())
        );
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn load_resolved_config_infers_container_workspace_from_mount_and_defaults() {
        let workspace = unique_temp_dir("devcontainer-config-resolution-workspace-mount");
        let config_dir = workspace.join(".devcontainer");
        fs::create_dir_all(&config_dir).expect("config dir");

        fs::write(
            config_dir.join("devcontainer.json"),
            r#"{
                "workspaceMount": "source=${localWorkspaceFolder},target=/custom,type=bind",
                "remoteEnv": {
                    "HERE": "${containerWorkspaceFolder}"
                }
            }"#,
        )
        .expect("config write");
        let (_, _, config) = load_resolved_config(&[
            "--workspace-folder".to_string(),
            workspace.display().to_string(),
        ])
        .expect("mount config");
        assert_eq!(config["remoteEnv"]["HERE"], "/custom");

        fs::write(
            config_dir.join("devcontainer.json"),
            r#"{
                "dockerComposeFile": "compose.yml",
                "service": "app",
                "remoteEnv": {
                    "HERE": "${containerWorkspaceFolder}"
                }
            }"#,
        )
        .expect("compose config write");
        let (_, _, config) = load_resolved_config(&[
            "--workspace-folder".to_string(),
            workspace.display().to_string(),
        ])
        .expect("compose config");
        assert_eq!(config["remoteEnv"]["HERE"], "/");

        fs::write(
            config_dir.join("devcontainer.json"),
            r#"{
                "image": "alpine",
                "remoteEnv": {
                    "HERE": "${containerWorkspaceFolder}"
                }
            }"#,
        )
        .expect("image config write");
        let (_, _, config) = load_resolved_config(&[
            "--workspace-folder".to_string(),
            workspace.display().to_string(),
        ])
        .expect("image config");
        assert_eq!(
            config["remoteEnv"]["HERE"],
            format!(
                "/workspaces/{}",
                workspace.file_name().unwrap().to_string_lossy()
            )
        );

        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn relative_override_config_paths_report_missing_files() {
        let error = resolve_override_config_path(&[
            "--override-config".to_string(),
            "missing-devcontainer.json".to_string(),
        ])
        .expect_err("missing override config");

        assert!(error.contains("missing-devcontainer.json"), "{error}");
    }
}
