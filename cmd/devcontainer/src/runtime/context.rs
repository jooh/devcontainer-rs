//! Runtime config loading and container-context resolution helpers.

mod inspection;
mod workspace;

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use crate::commands::{common, configuration};

use super::compose;
use super::container;
use inspection::{inspect_container_context, workspace_folder_from_args};

pub(crate) use workspace::{
    additional_mounts_for_workspace_target, combined_remote_env, configured_user,
    default_remote_workspace_folder, derived_workspace_mount, remote_user,
    remote_workspace_folder_for_args, workspace_mount_for_args,
};

#[derive(Debug)]
pub(crate) struct ResolvedConfig {
    pub(crate) workspace_folder: PathBuf,
    pub(crate) config_file: PathBuf,
    pub(crate) configuration: Value,
}

#[derive(Debug)]
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
            let configuration = configuration_with_feature_metadata(args, resolved)?;
            return Ok(ExistingContainerContext {
                container_id,
                configuration,
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
    let configuration = if let Some(resolved) = resolved.as_ref() {
        configuration_with_feature_metadata(args, resolved)?
    } else {
        inspected
            .as_ref()
            .expect("inspected context is available without resolved config")
            .configuration
            .clone()
    };
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

fn configuration_with_feature_metadata(
    args: &[String],
    resolved: &ResolvedConfig,
) -> Result<Value, String> {
    let feature_support = configuration::resolve_feature_support_without_lockfile(
        args,
        &resolved.workspace_folder,
        &resolved.config_file,
        &resolved.configuration,
    )?;
    Ok(feature_support
        .as_ref()
        .map(|resolved_features| {
            configuration::apply_feature_metadata(
                &resolved.configuration,
                &resolved_features.metadata_entries,
            )
        })
        .unwrap_or_else(|| resolved.configuration.clone()))
}

#[cfg(test)]
mod tests {
    //! Unit tests for runtime context helpers.

    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use serde_json::json;

    use crate::commands::common::DEVCONTAINER_LOCAL_FOLDER_LABEL;
    use crate::runtime::mounts::split_mount_options;
    use crate::test_support::{unique_temp_dir, write_executable_script};

    use super::{
        configuration_with_feature_metadata, default_remote_workspace_folder,
        derived_workspace_mount, load_optional_config, load_required_config,
        load_required_config_with_id_labels, remote_workspace_folder_for_args,
        resolve_existing_container_context, workspace_mount_for_args, ResolvedConfig,
    };

    #[test]
    fn existing_container_feature_metadata_ignores_corrupt_lockfile() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        let config_dir = root.join(".devcontainer");
        let feature_dir = config_dir.join("local-feature");
        fs::create_dir_all(&feature_dir).expect("failed to create feature directory");
        fs::write(
            feature_dir.join("devcontainer-feature.json"),
            "{\n  \"id\": \"local-feature\",\n  \"name\": \"Local Feature\",\n  \"version\": \"1.0.0\",\n  \"containerEnv\": {\n    \"LOCAL_FEATURE_ENV\": \"enabled\"\n  }\n}\n",
        )
        .expect("failed to write feature manifest");
        fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nset -eu\n")
            .expect("failed to write feature install script");
        let config_file = config_dir.join("devcontainer.json");
        fs::write(
            &config_file,
            "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"./local-feature\": {}\n  }\n}\n",
        )
        .expect("failed to write config");
        fs::write(
            config_dir.join("devcontainer-lock.json"),
            "this is not json",
        )
        .expect("failed to write corrupt lockfile");
        let resolved = ResolvedConfig {
            workspace_folder: root.clone(),
            config_file,
            configuration: json!({
                "image": "debian:bookworm",
                "features": {
                    "./local-feature": {},
                },
            }),
        };

        let configuration = configuration_with_feature_metadata(&[], &resolved)
            .expect("existing container metadata should not parse lockfile");

        assert_eq!(
            configuration["containerEnv"]["LOCAL_FEATURE_ENV"],
            "enabled"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn config_load_helpers_return_required_optional_and_legacy_label_configs() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");
        let config_file = write_workspace_config(
            &root,
            "{\n  \"image\": \"alpine:3.20\",\n  \"postAttachCommand\": \"echo ${devcontainerId}\"\n}\n",
        );
        let args = workspace_args(&root);

        let required = load_required_config(&args).expect("required config");
        let optional = load_optional_config(&args)
            .expect("optional config")
            .expect("resolved optional config");
        let legacy = load_required_config_with_id_labels(
            &args,
            HashMap::from([(
                DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
                root.display().to_string(),
            )]),
        )
        .expect("legacy labels config");

        assert_eq!(required.config_file, config_file);
        assert_eq!(optional.workspace_folder, required.workspace_folder);
        assert_eq!(required.configuration["image"], "alpine:3.20");
        assert_ne!(
            required.configuration["postAttachCommand"],
            legacy.configuration["postAttachCommand"]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_optional_config_skips_implicit_missing_config_but_reports_explicit_missing_config() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");

        assert!(load_optional_config(&workspace_args(&root))
            .expect("implicit missing config")
            .is_none());

        let missing_config = root.join("missing.json");
        let error = load_optional_config(&[
            "--workspace-folder".to_string(),
            root.display().to_string(),
            "--config".to_string(),
            missing_config.display().to_string(),
        ])
        .expect_err("explicit missing config should fail");

        assert!(error.starts_with("Unable to locate a dev container config at "));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_required_config_with_id_labels_reports_config_errors() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");
        let missing_config = root.join("missing.json");

        let error = expect_error(
            load_required_config_with_id_labels(
                &[
                    "--workspace-folder".to_string(),
                    root.display().to_string(),
                    "--config".to_string(),
                    missing_config.display().to_string(),
                ],
                HashMap::new(),
            ),
            "missing explicit config",
        );

        assert!(error.starts_with("Unable to locate a dev container config at "));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_existing_container_context_uses_compose_container_id() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");
        let config_file = write_workspace_config(
            &root,
            "{\n  \"dockerComposeFile\": \"compose.yml\",\n  \"service\": \"app\"\n}\n",
        );
        fs::write(
            config_file
                .parent()
                .expect("config parent")
                .join("compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let engine = root.join("engine");
        write_executable_script(
            &engine,
            "#!/bin/sh\ncase \"$1\" in\n  ps) printf 'compose-container\\n' ;;\n  *) exit 2 ;;\nesac\n",
        );
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));

        let context = resolve_existing_container_context(&args).expect("existing context");

        assert_eq!(context.container_id, "compose-container");
        assert_eq!(context.remote_workspace_folder, "/");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_existing_container_context_reports_compose_lookup_errors_and_missing_ids() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");
        let config_file = write_workspace_config(
            &root,
            "{\n  \"dockerComposeFile\": \"compose.yml\",\n  \"service\": \"app\"\n}\n",
        );
        fs::write(
            config_file
                .parent()
                .expect("config parent")
                .join("compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");

        let failing_engine = root.join("failing-engine");
        write_engine_script(
            &failing_engine,
            "#!/bin/sh\ncase \"$1\" in\n  ps) printf 'compose failed\\n' >&2; exit 7 ;;\n  *) exit 2 ;;\nesac\n",
        );
        let mut failing_args = workspace_args(&root);
        failing_args.extend(docker_args(&failing_engine));
        let failure = expect_error(
            resolve_existing_container_context(&failing_args),
            "compose ps failure",
        );
        assert_eq!(failure, "compose failed");

        let empty_engine = root.join("empty-engine");
        write_engine_script(
            &empty_engine,
            "#!/bin/sh\ncase \"$1\" in\n  ps) exit 0 ;;\n  *) exit 2 ;;\nesac\n",
        );
        let mut empty_args = workspace_args(&root);
        empty_args.extend(docker_args(&empty_engine));
        let missing = expect_error(
            resolve_existing_container_context(&empty_args),
            "missing compose container",
        );
        assert_eq!(missing, "Dev container not found.");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_existing_container_context_reports_compose_metadata_errors() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");
        let config_file = write_workspace_config(
            &root,
            "{\n  \"dockerComposeFile\": \"compose.yml\",\n  \"service\": \"app\"\n}\n",
        );
        fs::write(
            config_file
                .parent()
                .expect("config parent")
                .join("compose.yml"),
            "services:\n  app:\n    image: alpine:3.20\n",
        )
        .expect("compose file");
        let engine = root.join("engine");
        write_engine_script(
            &engine,
            "#!/bin/sh\ncase \"$1\" in\n  ps) printf 'compose-container\\n' ;;\n  *) exit 2 ;;\nesac\n",
        );
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));
        args.extend(["--additional-features".to_string(), "[]".to_string()]);

        let error = expect_error(
            resolve_existing_container_context(&args),
            "invalid additional features",
        );

        assert_eq!(error, "--additional-features must be a JSON object");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_existing_container_context_reloads_config_with_matched_legacy_id_labels() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");
        let _config_file = write_workspace_config(
            &root,
            "{\n  \"image\": \"alpine:3.20\",\n  \"postAttachCommand\": \"echo ${devcontainerId}\"\n}\n",
        );
        let canonical_root = fs::canonicalize(&root).expect("canonical workspace root");
        let engine = root.join("engine");
        let mut labels = serde_json::Map::new();
        labels.insert(
            DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
            json!(canonical_root.to_string_lossy()),
        );
        let payload = json!([{
            "Config": {
                "Labels": labels
            }
        }])
        .to_string();
        write_engine_script(
            &engine,
            &format!(
                "#!/bin/sh\ncase \"$1\" in\n  ps) printf 'container-id\\n' ;;\n  inspect) printf '%s\\n' '{}' ;;\n  *) exit 2 ;;\nesac\n",
                payload
            ),
        );
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));
        let legacy = load_required_config_with_id_labels(
            &args,
            HashMap::from([(
                DEVCONTAINER_LOCAL_FOLDER_LABEL.to_string(),
                canonical_root.display().to_string(),
            )]),
        )
        .expect("legacy labels config");

        let context = resolve_existing_container_context(&args).expect("existing context");

        assert_eq!(context.container_id, "container-id");
        assert_eq!(
            context.configuration["postAttachCommand"],
            legacy.configuration["postAttachCommand"]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_existing_container_context_reports_missing_matched_container() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");
        write_workspace_config(&root, "{\n  \"image\": \"alpine:3.20\"\n}\n");
        let engine = root.join("engine");
        write_engine_script(
            &engine,
            "#!/bin/sh\ncase \"$1\" in\n  ps) exit 0 ;;\n  *) exit 2 ;;\nesac\n",
        );
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));

        let error =
            resolve_existing_container_context(&args).expect_err("missing container should fail");

        assert_eq!(error, "Dev container not found.");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_existing_container_context_reports_optional_config_errors() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");
        let missing_config = root.join("missing.json");

        let error = expect_error(
            resolve_existing_container_context(&[
                "--workspace-folder".to_string(),
                root.display().to_string(),
                "--config".to_string(),
                missing_config.display().to_string(),
                "--container-id".to_string(),
                "container-id".to_string(),
            ]),
            "explicit config failure",
        );

        assert!(error.starts_with("Unable to locate a dev container config at "));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_existing_container_context_reports_inspection_errors_without_config() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");
        let engine = root.join("engine");
        write_engine_script(
            &engine,
            "#!/bin/sh\ncase \"$1\" in\n  ps) printf 'container-id\\n' ;;\n  inspect) printf 'inspect failed\\n' >&2; exit 7 ;;\n  *) exit 2 ;;\nesac\n",
        );
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));
        args.extend([
            "--id-label".to_string(),
            "devcontainer.test=true".to_string(),
        ]);

        let error = expect_error(
            resolve_existing_container_context(&args),
            "inspect context failure",
        );

        assert_eq!(error, "inspect failed");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_existing_container_context_reports_feature_metadata_errors() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");
        write_workspace_config(&root, "{\n  \"image\": \"alpine:3.20\"\n}\n");
        let engine = root.join("engine");
        write_engine_script(
            &engine,
            "#!/bin/sh\ncase \"$1\" in\n  inspect) printf '[{\"Config\":{\"Labels\":{}}}]\\n' ;;\n  *) exit 2 ;;\nesac\n",
        );
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));
        args.extend([
            "--container-id".to_string(),
            "container-id".to_string(),
            "--additional-features".to_string(),
            "[]".to_string(),
        ]);

        let error = expect_error(
            resolve_existing_container_context(&args),
            "feature metadata failure",
        );

        assert_eq!(error, "--additional-features must be a JSON object");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_existing_container_context_inspects_explicit_container_without_config() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");
        let engine = root.join("engine");
        let payload = json!([{
            "Config": {
                "Labels": {},
                "User": "vscode"
            },
            "Mounts": [{
                "Destination": "/inspected"
            }]
        }])
        .to_string();
        write_inspect_engine(&engine, &payload);
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));
        args.extend(["--container-id".to_string(), "container-id".to_string()]);

        let context = resolve_existing_container_context(&args).expect("existing context");

        assert_eq!(context.container_id, "container-id");
        assert_eq!(context.configuration["containerUser"], "vscode");
        assert_eq!(context.remote_workspace_folder, "/inspected");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resolve_existing_container_context_defaults_remote_workspace_without_inspected_mount() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");
        let engine = root.join("engine");
        let payload = json!([{
            "Config": {
                "Labels": {},
                "User": ""
            },
            "Mounts": []
        }])
        .to_string();
        write_inspect_engine(&engine, &payload);
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));
        args.extend(["--container-id".to_string(), "container-id".to_string()]);

        let context = resolve_existing_container_context(&args).expect("existing context");

        assert_eq!(
            context.remote_workspace_folder,
            default_remote_workspace_folder(Some(
                fs::canonicalize(&root)
                    .expect("canonical workspace root")
                    .as_path()
            ))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn configuration_with_feature_metadata_reports_additional_feature_parse_errors() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        fs::create_dir_all(&root).expect("workspace");
        let config_file = write_workspace_config(&root, "{\n  \"image\": \"alpine:3.20\"\n}\n");
        let resolved = ResolvedConfig {
            workspace_folder: root.clone(),
            config_file,
            configuration: json!({
                "image": "alpine:3.20"
            }),
        };

        let error = configuration_with_feature_metadata(
            &["--additional-features".to_string(), "[]".to_string()],
            &resolved,
        )
        .expect_err("invalid additional features should fail");

        assert_eq!(error, "--additional-features must be a JSON object");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn compose_remote_workspace_folder_prefers_configured_workspace_folder() {
        let resolved = ResolvedConfig {
            workspace_folder: std::path::PathBuf::from("/tmp/example"),
            config_file: std::path::PathBuf::from("/tmp/example/.devcontainer/devcontainer.json"),
            configuration: json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app",
                "workspaceFolder": "/configured"
            }),
        };

        assert_eq!(
            remote_workspace_folder_for_args(&resolved, &[]),
            "/configured"
        );
    }

    #[test]
    fn compose_remote_workspace_folder_defaults_to_root() {
        let resolved = ResolvedConfig {
            workspace_folder: std::path::PathBuf::from("/tmp/example"),
            config_file: std::path::PathBuf::from("/tmp/example/.devcontainer/devcontainer.json"),
            configuration: json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        };

        assert_eq!(remote_workspace_folder_for_args(&resolved, &[]), "/");
    }

    #[test]
    fn compose_remote_workspace_folder_ignores_workspace_mount() {
        let resolved = ResolvedConfig {
            workspace_folder: std::path::PathBuf::from("/tmp/example"),
            config_file: std::path::PathBuf::from("/tmp/example/.devcontainer/devcontainer.json"),
            configuration: json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app",
                "workspaceMount": "type=bind,source=/tmp/example,target=/mounted"
            }),
        };

        assert_eq!(remote_workspace_folder_for_args(&resolved, &[]), "/");
    }

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
        #[cfg(target_os = "linux")]
        assert!(!mount.contains("consistency="));
        #[cfg(not(target_os = "linux"))]
        assert!(mount.contains("consistency=delegated"));
    }

    #[test]
    fn workspace_mount_for_args_preserves_git_root_source_for_nested_workspace_folder_targets() {
        let root = unique_temp_dir("devcontainer-runtime-context");
        let repo_root = root.join("repo");
        let workspace = repo_root.join("packages").join("app");
        fs::create_dir_all(workspace.join(".devcontainer")).expect("config dir");
        init_git_repo(&repo_root);
        let expected_repo_root = repo_root.canonicalize().expect("canonical repo root");
        let resolved = ResolvedConfig {
            workspace_folder: workspace.canonicalize().expect("canonical workspace"),
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

    fn workspace_args(workspace: &Path) -> Vec<String> {
        vec![
            "--workspace-folder".to_string(),
            workspace.display().to_string(),
        ]
    }

    fn docker_args(engine: &Path) -> Vec<String> {
        vec!["--docker-path".to_string(), engine.display().to_string()]
    }

    fn write_workspace_config(workspace: &Path, contents: &str) -> PathBuf {
        let config_dir = workspace.join(".devcontainer");
        fs::create_dir_all(&config_dir).expect("config dir");
        let config_file = config_dir.join("devcontainer.json");
        fs::write(&config_file, contents).expect("config write");
        fs::canonicalize(&config_file).unwrap_or(config_file)
    }

    fn write_inspect_engine(engine: &Path, payload: &str) {
        write_engine_script(
            engine,
            &format!(
                "#!/bin/sh\ncase \"$1\" in\n  inspect) printf '%s\\n' '{}' ;;\n  *) exit 2 ;;\nesac\n",
                payload
            ),
        );
    }

    fn write_engine_script(engine: &Path, script: &str) {
        write_executable_script(engine, script);
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
