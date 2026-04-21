//! Runtime config loading and container-context resolution helpers.

mod inspection;
mod workspace;

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use crate::commands::common;

use super::compose;
use super::container;
use inspection::{inspect_container_context, workspace_folder_from_args};

pub(crate) use workspace::{
    additional_mounts_for_workspace_target, combined_remote_env, configured_user,
    default_remote_workspace_folder, derived_workspace_mount, remote_user,
    remote_workspace_folder_for_args, workspace_mount_for_args,
};

pub(crate) struct ResolvedConfig {
    pub(crate) workspace_folder: PathBuf,
    pub(crate) config_file: PathBuf,
    pub(crate) configuration: Value,
}

pub(crate) struct ExistingContainerContext {
    pub(crate) container_id: String,
    pub(crate) configuration: Value,
    pub(crate) remote_workspace_folder: String,
}

pub(crate) struct DerivedWorkspaceMount {
    pub(crate) host_mount_folder: PathBuf,
    pub(crate) container_mount_folder: String,
    pub(crate) remote_workspace_folder: String,
    pub(crate) additional_mounts: Vec<String>,
}

pub(crate) fn load_required_config(args: &[String]) -> Result<ResolvedConfig, String> {
    let (workspace_folder, config_file, configuration) = common::load_resolved_config(args)?;
    Ok(ResolvedConfig {
        workspace_folder,
        config_file,
        configuration,
    })
}

pub(crate) fn load_required_config_with_id_labels(
    args: &[String],
    id_labels: HashMap<String, String>,
) -> Result<ResolvedConfig, String> {
    let (workspace_folder, config_file, configuration) =
        common::load_resolved_config_with_id_labels(args, id_labels)?;
    Ok(ResolvedConfig {
        workspace_folder,
        config_file,
        configuration,
    })
}

pub(crate) fn load_optional_config(args: &[String]) -> Result<Option<ResolvedConfig>, String> {
    let explicit_config = common::parse_option_value(args, "--config");
    match load_required_config(args) {
        Ok(config) => Ok(Some(config)),
        Err(error)
            if explicit_config.is_none()
                && error.starts_with("Unable to locate a dev container config at ") =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn resolve_existing_container_context(
    args: &[String],
) -> Result<ExistingContainerContext, String> {
    let resolved = load_optional_config(args)?;
    let explicit_container_id = common::parse_option_value(args, "--container-id");
    if let Some(resolved) = &resolved {
        if explicit_container_id.is_none() && compose::uses_compose_config(&resolved.configuration)
        {
            let container_id = compose::resolve_container_id(resolved, args)?
                .ok_or_else(|| "Dev container not found.".to_string())?;
            return Ok(ExistingContainerContext {
                container_id,
                configuration: resolved.configuration.clone(),
                remote_workspace_folder: remote_workspace_folder_for_args(resolved, args),
            });
        }
    }
    let workspace_folder = if let Some(resolved) = &resolved {
        Some(resolved.workspace_folder.clone())
    } else {
        workspace_folder_from_args(args)?
    };
    let container::ResolvedTargetContainer {
        container_id,
        id_labels,
    } = container::resolve_target_container_match(
        args,
        resolved
            .as_ref()
            .map(|value| value.workspace_folder.as_path())
            .or(workspace_folder.as_deref()),
        resolved.as_ref().map(|value| value.config_file.as_path()),
    )?;
    let resolved = match (resolved, id_labels) {
        (Some(_), Some(id_labels)) => Some(load_required_config_with_id_labels(args, id_labels)?),
        (resolved, _) => resolved,
    };
    let inspected = if resolved.is_none() {
        Some(inspect_container_context(args, &container_id)?)
    } else {
        None
    };
    let configuration = resolved
        .as_ref()
        .map(|value| value.configuration.clone())
        .or_else(|| inspected.as_ref().map(|value| value.configuration.clone()))
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    let remote_workspace_folder = resolved
        .as_ref()
        .map(|resolved| remote_workspace_folder_for_args(resolved, args))
        .or_else(|| {
            inspected
                .as_ref()
                .and_then(|value| value.remote_workspace_folder.clone())
        })
        .unwrap_or_else(|| {
            default_remote_workspace_folder(
                inspected
                    .as_ref()
                    .and_then(|value| value.local_workspace_folder.as_deref())
                    .or(workspace_folder.as_deref()),
            )
        });

    Ok(ExistingContainerContext {
        container_id,
        configuration,
        remote_workspace_folder,
    })
}

#[cfg(test)]
mod tests {
    //! Unit tests for runtime context helpers.

    use std::fs;
    use std::process::Command;

    use serde_json::json;

    use crate::runtime::mounts::split_mount_options;
    use crate::test_support::unique_temp_dir;

    use super::{
        default_remote_workspace_folder, derived_workspace_mount, remote_workspace_folder_for_args,
        workspace_mount_for_args, ResolvedConfig,
    };

    #[test]
    fn remote_workspace_folder_prefers_configured_workspace_folder() {
        let resolved = ResolvedConfig {
            workspace_folder: std::path::PathBuf::from("/tmp/example"),
            config_file: std::path::PathBuf::from("/tmp/example/.devcontainer/devcontainer.json"),
            configuration: json!({
                "workspaceFolder": "/configured"
            }),
        };

        assert_eq!(
            remote_workspace_folder_for_args(&resolved, &[]),
            "/configured"
        );
    }

    #[test]
    fn remote_workspace_folder_falls_back_to_workspace_mount_target() {
        let resolved = ResolvedConfig {
            workspace_folder: std::path::PathBuf::from("/tmp/example"),
            config_file: std::path::PathBuf::from("/tmp/example/.devcontainer/devcontainer.json"),
            configuration: json!({
                "workspaceMount": "type=bind,source=/tmp/example,target=/mounted"
            }),
        };

        assert_eq!(remote_workspace_folder_for_args(&resolved, &[]), "/mounted");
    }

    #[test]
    fn default_remote_workspace_folder_uses_workspace_basename() {
        assert_eq!(
            default_remote_workspace_folder(Some(std::path::Path::new("/tmp/project"))),
            "/workspaces/project"
        );
    }

    #[test]
    fn workspace_mount_for_args_adds_requested_consistency_on_non_linux_hosts() {
        let resolved = ResolvedConfig {
            workspace_folder: std::path::PathBuf::from("/tmp/example"),
            config_file: std::path::PathBuf::from("/tmp/example/.devcontainer/devcontainer.json"),
            configuration: json!({}),
        };
        let mount = workspace_mount_for_args(
            &resolved,
            "/workspaces/example",
            &[
                "--workspace-mount-consistency".to_string(),
                "delegated".to_string(),
            ],
        );
        if std::env::consts::OS == "linux" {
            assert!(!mount.contains("consistency="));
        } else {
            assert!(mount.contains("consistency=delegated"));
        }
    }

    #[test]
    fn workspace_mount_for_args_preserves_git_root_source_for_nested_workspace_folder_targets() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        let repo_root = root.join("repo");
        let workspace = repo_root.join("packages").join("app");
        fs::create_dir_all(workspace.join(".devcontainer")).expect("config dir");
        init_git_repo(&repo_root);
        let expected_repo_root = repo_root
            .canonicalize()
            .unwrap_or_else(|_| repo_root.clone());
        let resolved = ResolvedConfig {
            workspace_folder: workspace
                .canonicalize()
                .unwrap_or_else(|_| workspace.clone()),
            config_file: workspace.join(".devcontainer").join("devcontainer.json"),
            configuration: json!({
                "workspaceFolder": "/workspace"
            }),
        };

        let mount = workspace_mount_for_args(&resolved, "/workspace", &[]);
        let options = split_mount_options(&mount);

        assert!(options.contains(&format!("source={}", expected_repo_root.display())));
        assert!(!options.contains(&format!("source={}", resolved.workspace_folder.display())));
        assert!(options.contains(&"target=/workspace".to_string()));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn derived_workspace_mount_uses_workspace_folder_when_git_root_mount_is_disabled() {
        let workspace = std::env::temp_dir().join("devcontainer-rs-no-git-root");
        let derived = derived_workspace_mount(
            &workspace,
            &[
                "--mount-workspace-git-root".to_string(),
                "false".to_string(),
            ],
        )
        .expect("derived mount");
        assert_eq!(derived.host_mount_folder, workspace);
        assert_eq!(
            derived.remote_workspace_folder,
            "/workspaces/devcontainer-rs-no-git-root"
        );
        assert!(derived.additional_mounts.is_empty());
    }

    fn init_git_repo(root: &std::path::Path) {
        let status = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root)
            .status()
            .expect("git init");
        assert!(status.success(), "git init failed: {status:?}");
    }
}
