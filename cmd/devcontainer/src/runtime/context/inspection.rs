//! Container inspection helpers for deriving runtime context from existing containers.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
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
        let inspected_user = details
            .get("Config")
            .and_then(|value| value.get("User"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        if let (Some(user), Value::Object(entries)) = (inspected_user, &mut configuration) {
            entries.insert("containerUser".to_string(), Value::String(user.to_string()));
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
    workspace_folder_from_args_with_current_dir(args, env::current_dir)
}

fn workspace_folder_from_args_with_current_dir(
    args: &[String],
    current_dir: impl FnOnce() -> io::Result<PathBuf>,
) -> Result<Option<PathBuf>, String> {
    if let Some(workspace_folder) = common::parse_option_value(args, "--workspace-folder") {
        return Ok(Some(
            fs::canonicalize(&workspace_folder).unwrap_or_else(|_| PathBuf::from(workspace_folder)),
        ));
    }
    let workspace_folder = current_dir().map_err(|error| error.to_string())?;
    Ok(Some(workspace_folder))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::Path;

    use serde_json::json;

    use super::{
        inspect_container_context, inspect_workspace_mount, workspace_folder_from_args,
        workspace_folder_from_args_with_current_dir,
    };
    use crate::test_support::{unique_temp_dir, write_executable_script};

    #[test]
    fn inspect_container_context_reports_engine_stderr() {
        let root = unique_temp_dir("devcontainer-inspection-test");
        fs::create_dir_all(&root).expect("temp root");
        let engine = root.join("engine");
        write_executable_script(
            &engine,
            "#!/bin/sh\nprintf 'inspect failed\\n' >&2\nexit 7\n",
        );
        let args = docker_args(&engine);

        let error = inspect_container_context(&args, "container-id")
            .err()
            .expect("inspect failure");
        assert_eq!(error, "inspect failed");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn inspect_container_context_reports_engine_spawn_errors() {
        let error = expect_error(
            inspect_container_context(
                &[
                    "--docker-path".to_string(),
                    "/definitely/missing/container-engine".to_string(),
                ],
                "container-id",
            ),
            "engine spawn failure",
        );

        assert!(error.contains("Container engine executable not found"));
    }

    #[test]
    fn inspect_container_context_rejects_invalid_or_empty_inspect_json() {
        let invalid = inspect_context_with_payload("not json")
            .err()
            .expect("invalid json");
        assert!(invalid.contains("Invalid inspect JSON"));

        let empty = inspect_context_with_payload("[]")
            .err()
            .expect("empty inspect result");
        assert_eq!(empty, "Container engine did not return inspect details");
    }

    #[test]
    fn inspect_container_context_merges_metadata_labels_and_container_user() {
        let root = unique_temp_dir("devcontainer-inspection-test");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let engine = root.join("engine");
        let metadata = json!({
            "workspaceFolder": "${localWorkspaceFolder}/inside",
            "remoteEnv": {
                "FROM_METADATA": "yes"
            }
        })
        .to_string();
        let payload = json!([{
            "Config": {
                "Labels": {
                    "devcontainer.local_folder": workspace.to_string_lossy(),
                    "devcontainer.metadata": metadata
                },
                "User": "vscode"
            },
            "Mounts": [{
                "Source": workspace.to_string_lossy(),
                "Destination": "/workspace"
            }]
        }])
        .to_string();
        write_inspect_script(&engine, &payload);
        let args = docker_args(&engine);

        let context = inspect_container_context(&args, "container-id").expect("inspect context");

        assert_eq!(
            context.local_workspace_folder.as_deref(),
            Some(workspace.as_path())
        );
        assert_eq!(context.configuration["containerUser"], "vscode");
        assert_eq!(context.configuration["remoteEnv"]["FROM_METADATA"], "yes");
        assert_eq!(
            context.remote_workspace_folder.as_deref(),
            Some(format!("{}/inside", workspace.display()).as_str())
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn inspect_container_context_uses_mount_destination_without_metadata_workspace() {
        let payload = json!([{
            "Config": {
                "Labels": {},
                "User": "vscode"
            },
            "Mounts": [{
                "Destination": "/fallback"
            }]
        }])
        .to_string();

        let context = inspect_context_with_payload(&payload).expect("inspect context");

        assert_eq!(context.configuration["containerUser"], "vscode");
        assert_eq!(
            context.remote_workspace_folder.as_deref(),
            Some("/fallback")
        );
    }

    #[test]
    fn inspect_container_context_preserves_metadata_user_over_inspected_user() {
        let metadata = json!({
            "remoteUser": "metadata-user"
        })
        .to_string();
        let payload = json!([{
            "Config": {
                "Labels": {
                    "devcontainer.metadata": metadata
                },
                "User": "image-user"
            },
            "Mounts": [{
                "Destination": "/workspace"
            }]
        }])
        .to_string();

        let context = inspect_context_with_payload(&payload).expect("inspect context");

        assert_eq!(context.configuration["remoteUser"], "metadata-user");
        assert!(context.configuration.get("containerUser").is_none());
    }

    #[test]
    fn inspect_workspace_mount_prefers_matching_local_source() {
        let details = json!({
            "Mounts": [
                {
                    "Source": "/other",
                    "Destination": "/other-container"
                },
                {
                    "Source": "/host/project",
                    "Destination": "/workspace/project"
                }
            ]
        });

        assert_eq!(
            inspect_workspace_mount(&details, Some(Path::new("/host/project"))).as_deref(),
            Some("/workspace/project")
        );
    }

    #[test]
    fn inspect_workspace_mount_falls_back_to_first_destination() {
        let details = json!({
            "Mounts": [
                {
                    "Source": "/other",
                    "Destination": "/fallback"
                }
            ]
        });

        assert_eq!(
            inspect_workspace_mount(&details, Some(Path::new("/host/project"))).as_deref(),
            Some("/fallback")
        );
        assert_eq!(
            inspect_workspace_mount(&json!({}), Some(Path::new("/host/project"))),
            None
        );
    }

    #[test]
    fn workspace_folder_from_args_preserves_missing_explicit_workspace() {
        let root = unique_temp_dir("devcontainer-inspection-test");
        let missing = root.join("missing");
        let args = vec![
            "--workspace-folder".to_string(),
            missing.to_string_lossy().to_string(),
        ];

        assert_eq!(
            workspace_folder_from_args(&args).expect("workspace folder"),
            Some(missing)
        );
        assert!(workspace_folder_from_args(&[])
            .expect("current workspace")
            .is_some());
    }

    #[test]
    fn workspace_folder_from_args_reports_deleted_current_dir() {
        let error = workspace_folder_from_args_with_current_dir(&[], || {
            Err(io::Error::new(io::ErrorKind::NotFound, "deleted cwd"))
        })
        .expect_err("deleted current dir");

        assert_eq!(error, "deleted cwd");
    }

    fn inspect_context_with_payload(
        payload: &str,
    ) -> Result<super::InspectedContainerContext, String> {
        let root = unique_temp_dir("devcontainer-inspection-test");
        fs::create_dir_all(&root).expect("temp root");
        let engine = root.join("engine");
        write_inspect_script(&engine, payload);
        let result = inspect_container_context(&docker_args(&engine), "container-id");
        let _ = fs::remove_dir_all(root);
        result
    }

    fn write_inspect_script(engine: &Path, payload: &str) {
        write_executable_script(
            engine,
            &format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", payload),
        );
    }

    fn docker_args(engine: &Path) -> Vec<String> {
        vec![
            "--docker-path".to_string(),
            engine.to_string_lossy().to_string(),
        ]
    }

    fn expect_error<T>(result: Result<T, String>, context: &str) -> String {
        match result {
            Ok(_) => panic!("expected {context}"),
            Err(error) => error,
        }
    }

    #[test]
    fn expect_error_helper_panics_for_unexpected_success() {
        let panic = std::panic::catch_unwind(|| {
            let _ = expect_error(Ok::<(), String>(()), "unexpected success");
        });

        assert!(panic.is_err());
    }
}
