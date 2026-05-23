//! Public runtime command orchestration for build, up, exec, and lifecycle flows.

mod build;
pub(crate) mod compose;
mod container;
pub(crate) mod context;
mod dockerfile;
pub(crate) mod engine;
mod exec;
mod image;
mod lifecycle;
pub(crate) mod metadata;
pub(crate) mod mounts;
mod paths;
mod user_resolution;

use serde_json::{json, Value};

use crate::commands::common;
use crate::commands::configuration;

fn effective_up_resolved_config(
    args: &[String],
    resolved: context::ResolvedConfig,
) -> Result<context::ResolvedConfig, String> {
    let feature_support = configuration::resolve_feature_support(
        args,
        &resolved.workspace_folder,
        &resolved.config_file,
        &resolved.configuration,
    )?;
    let effective_configuration = match &feature_support {
        Some(resolved_features) => configuration::apply_feature_metadata(
            &resolved.configuration,
            &resolved_features.metadata_entries,
        ),
        None => resolved.configuration.clone(),
    };
    Ok(context::ResolvedConfig {
        workspace_folder: resolved.workspace_folder,
        config_file: resolved.config_file,
        configuration: effective_configuration,
    })
}

pub fn run_build(args: &[String]) -> Result<Value, String> {
    configuration::validate_lockfile_options(args)?;
    configuration::warn_deprecated_lockfile_flags(args);
    let resolved = context::load_required_config(args)?;
    let feature_support = configuration::resolve_feature_support(
        args,
        &resolved.workspace_folder,
        &resolved.config_file,
        &resolved.configuration,
    )?;
    let skip_feature_customizations =
        common::runtime_options(args).skip_persisting_customizations_from_features;
    let effective_configuration = match &feature_support {
        Some(resolved_features) => configuration::apply_feature_metadata_with_options(
            &resolved.configuration,
            &resolved_features.metadata_entries,
            skip_feature_customizations,
        ),
        None => resolved.configuration.clone(),
    };
    let image_name = build::build_image(&resolved, args)?;

    Ok(json!({
        "outcome": "success",
        "command": "build",
        "workspaceFolder": resolved.workspace_folder,
        "configFile": resolved.config_file,
        "imageName": image_name,
        "configuration": effective_configuration,
    }))
}

pub fn run_up(args: &[String]) -> Result<Value, String> {
    configuration::validate_lockfile_options(args)?;
    configuration::warn_deprecated_lockfile_flags(args);
    let _ = mounts::cli_mount_values(args)?;
    let resolved = context::load_required_config(args)?;
    let effective_resolved = effective_up_resolved_config(args, resolved)?;
    let effective_resolved =
        match container::probe_up_container_id_labels(&effective_resolved, args)? {
            Some(id_labels) => {
                let resolved = context::load_required_config_with_id_labels(args, id_labels)?;
                effective_up_resolved_config(args, resolved)?
            }
            None => effective_resolved,
        };
    lifecycle::run_initialize_command(
        args,
        &effective_resolved.configuration,
        &effective_resolved.workspace_folder,
    )?;
    let compose_project_name =
        compose::load_compose_spec(&effective_resolved)?.map(|spec| spec.project_name);
    let image_name = build::runtime_image_name(&effective_resolved, args)?;
    let image_name = container::prepare_up_image(&effective_resolved, args, &image_name)?;
    let remote_workspace_folder =
        context::remote_workspace_folder_for_args(&effective_resolved, args);
    let up_container = container::ensure_up_container(
        &effective_resolved,
        args,
        &image_name,
        &remote_workspace_folder,
    )?;
    let lifecycle_resolved = match up_container.matched_id_labels.clone() {
        Some(id_labels) => {
            let resolved = context::load_required_config_with_id_labels(args, id_labels)?;
            effective_up_resolved_config(args, resolved)?
        }
        None => effective_resolved,
    };
    let remote_workspace_folder =
        context::remote_workspace_folder_for_args(&lifecycle_resolved, args);
    lifecycle::run_lifecycle_commands(
        &up_container.container_id,
        args,
        &lifecycle_resolved.configuration,
        &remote_workspace_folder,
        up_container.lifecycle_mode,
    )?;

    Ok(json!({
        "outcome": "success",
        "command": "up",
        "containerId": up_container.container_id,
        "composeProjectName": compose_project_name,
        "remoteUser": context::remote_user(&lifecycle_resolved.configuration),
        "remoteWorkspaceFolder": remote_workspace_folder,
        "configuration": if common::has_flag(args, "--include-configuration") { lifecycle_resolved.configuration.clone() } else { Value::Null },
        "mergedConfiguration": if common::has_flag(args, "--include-merged-configuration") { lifecycle_resolved.configuration.clone() } else { Value::Null },
        "workspaceFolder": lifecycle_resolved.workspace_folder,
        "configFile": lifecycle_resolved.config_file,
    }))
}

pub fn run_set_up(args: &[String]) -> Result<Value, String> {
    let context = context::resolve_existing_container_context(args)?;
    lifecycle::run_lifecycle_commands(
        &context.container_id,
        args,
        &context.configuration,
        &context.remote_workspace_folder,
        lifecycle::LifecycleMode::SetUp,
    )?;

    Ok(json!({
        "outcome": "success",
        "command": "set-up",
        "containerId": context.container_id,
        "configuration": if common::has_flag(args, "--include-configuration") { context.configuration.clone() } else { Value::Null },
        "mergedConfiguration": if common::has_flag(args, "--include-merged-configuration") { context.configuration } else { Value::Null },
        "remoteWorkspaceFolder": context.remote_workspace_folder,
    }))
}

pub fn run_user_commands(args: &[String]) -> Result<Value, String> {
    let context = context::resolve_existing_container_context(args)?;
    lifecycle::run_lifecycle_commands(
        &context.container_id,
        args,
        &context.configuration,
        &context.remote_workspace_folder,
        lifecycle::LifecycleMode::RunUserCommands,
    )?;

    Ok(json!({
        "outcome": "success",
        "command": "run-user-commands",
        "containerId": context.container_id,
        "remoteWorkspaceFolder": context.remote_workspace_folder,
    }))
}

pub fn run_exec(args: &[String]) -> Result<i32, String> {
    let command_args = exec::exec_command_and_args(args)?;
    let context = context::resolve_existing_container_context(args)?;
    let engine_args = exec::exec_engine_args(
        args,
        &context.configuration,
        &context.remote_workspace_folder,
        &context.container_id,
        command_args,
        exec::ExecStdio::current(),
    )?;

    engine::run_engine_streaming(args, engine_args)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::test_support::{unique_temp_dir, write_executable_script};

    use super::{run_exec, run_set_up, run_up, run_user_commands};

    #[test]
    fn run_up_reports_lockfile_option_errors() {
        let error = run_up(&["--no-lockfile".to_string(), "--frozen-lockfile".to_string()])
            .expect_err("lockfile option error");

        assert_eq!(
            error,
            "--no-lockfile and --frozen-lockfile are mutually exclusive."
        );
    }

    #[test]
    fn run_up_reports_feature_resolution_errors() {
        let root = unique_temp_dir("devcontainer-runtime-mod-test");
        fs::create_dir_all(&root).expect("workspace");
        write_workspace_config(
            &root,
            "{\n  \"image\": \"alpine:3.20\",\n  \"updateRemoteUserUID\": false\n}\n",
        );
        let mut args = workspace_args(&root);
        args.extend(["--additional-features".to_string(), "[]".to_string()]);

        let error = run_up(&args).expect_err("feature resolution error");

        assert_eq!(error, "--additional-features must be a JSON object");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_up_reports_initialize_command_errors() {
        let root = unique_temp_dir("devcontainer-runtime-mod-test");
        fs::create_dir_all(&root).expect("workspace");
        write_workspace_config(
            &root,
            "{\n  \"image\": \"alpine:3.20\",\n  \"updateRemoteUserUID\": false,\n  \"initializeCommand\": \"printf 'initialize failed' >&2; exit 8\"\n}\n",
        );
        let engine = root.join("engine");
        write_existing_container_engine(&engine, None);
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));

        let error = run_up(&args).expect_err("initialize failure");

        assert_eq!(error, "initialize failed");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_up_reports_runtime_image_name_errors() {
        let root = unique_temp_dir("devcontainer-runtime-mod-test");
        fs::create_dir_all(&root).expect("workspace");
        write_workspace_config(&root, "{\n  \"updateRemoteUserUID\": false\n}\n");
        let engine = root.join("engine");
        write_existing_container_engine(&engine, None);
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));

        let error = run_up(&args).expect_err("runtime image name failure");

        assert_eq!(
            error,
            "Unsupported configuration: only image and build-based configs are supported natively"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_up_reports_lifecycle_errors() {
        let root = unique_temp_dir("devcontainer-runtime-mod-test");
        fs::create_dir_all(&root).expect("workspace");
        write_workspace_config(
            &root,
            "{\n  \"image\": \"alpine:3.20\",\n  \"updateRemoteUserUID\": false,\n  \"postAttachCommand\": \"echo attach\"\n}\n",
        );
        let engine = root.join("engine");
        write_existing_container_engine(&engine, Some(12));
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));

        let error = run_up(&args).expect_err("lifecycle failure");

        assert_eq!(error, "lifecycle failed");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_up_includes_merged_configuration_when_requested() {
        let root = unique_temp_dir("devcontainer-runtime-mod-test");
        fs::create_dir_all(&root).expect("workspace");
        write_workspace_config(
            &root,
            "{\n  \"image\": \"alpine:3.20\",\n  \"remoteUser\": \"vscode\",\n  \"updateRemoteUserUID\": false\n}\n",
        );
        let engine = root.join("engine");
        write_existing_container_engine(&engine, None);
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));
        args.push("--include-merged-configuration".to_string());

        let output = run_up(&args).expect("up success");

        assert_eq!(output["mergedConfiguration"]["remoteUser"], "vscode");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_up_reloads_configuration_for_matched_default_labels() {
        let root = unique_temp_dir("devcontainer-runtime-mod-test");
        fs::create_dir_all(&root).expect("workspace");
        let config_file = write_workspace_config(
            &root,
            "{\n  \"image\": \"alpine:3.20\",\n  \"remoteUser\": \"vscode\",\n  \"updateRemoteUserUID\": false\n}\n",
        );
        let engine = root.join("engine");
        write_labeled_existing_container_engine(&engine, &root, &config_file);
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));
        args.push("--include-merged-configuration".to_string());

        let output = run_up(&args).expect("up success");

        assert_eq!(output["containerId"], "container-id");
        assert_eq!(output["mergedConfiguration"]["remoteUser"], "vscode");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_up_reports_probe_label_feature_reload_errors() {
        let root = unique_temp_dir("devcontainer-runtime-mod-test");
        fs::create_dir_all(&root).expect("workspace");
        let config_file = write_workspace_config(
            &root,
            "{\n  \"image\": \"alpine:3.20\",\n  \"updateRemoteUserUID\": false\n}\n",
        );
        let engine = root.join("engine");
        write_config_mutating_label_engine(&engine, &root, &config_file, 1);
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));

        let error = run_up(&args).expect_err("feature reload failure");

        assert!(error.contains("No such file"), "{error}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_up_reports_lifecycle_label_feature_reload_errors() {
        let root = unique_temp_dir("devcontainer-runtime-mod-test");
        fs::create_dir_all(&root).expect("workspace");
        let config_file = write_workspace_config(
            &root,
            "{\n  \"image\": \"alpine:3.20\",\n  \"updateRemoteUserUID\": false\n}\n",
        );
        let engine = root.join("engine");
        write_config_mutating_label_engine(&engine, &root, &config_file, 2);
        let mut args = workspace_args(&root);
        args.extend(docker_args(&engine));

        let error = run_up(&args).expect_err("lifecycle feature reload failure");

        assert!(error.contains("No such file"), "{error}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_set_up_reports_lifecycle_errors_and_includes_merged_configuration() {
        let root = unique_temp_dir("devcontainer-runtime-mod-test");
        fs::create_dir_all(&root).expect("workspace");
        write_workspace_config(
            &root,
            "{\n  \"image\": \"alpine:3.20\",\n  \"postAttachCommand\": \"echo attach\"\n}\n",
        );
        let engine = root.join("engine");
        write_existing_container_engine(&engine, Some(12));
        let mut args = existing_container_args(&root, &engine);

        let error = run_set_up(&args).expect_err("set-up lifecycle failure");
        assert_eq!(error, "lifecycle failed");

        write_workspace_config(
            &root,
            "{\n  \"image\": \"alpine:3.20\",\n  \"remoteUser\": \"vscode\"\n}\n",
        );
        args.push("--include-merged-configuration".to_string());
        let output = run_set_up(&args).expect("set-up success");
        assert_eq!(output["mergedConfiguration"]["remoteUser"], "vscode");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_user_commands_reports_lifecycle_errors() {
        let root = unique_temp_dir("devcontainer-runtime-mod-test");
        fs::create_dir_all(&root).expect("workspace");
        write_workspace_config(
            &root,
            "{\n  \"image\": \"alpine:3.20\",\n  \"postAttachCommand\": \"echo attach\"\n}\n",
        );
        let engine = root.join("engine");
        write_existing_container_engine(&engine, Some(12));
        let args = existing_container_args(&root, &engine);

        let error = run_user_commands(&args).expect_err("user command lifecycle failure");

        assert_eq!(error, "lifecycle failed");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_user_commands_reports_success_payload() {
        let root = unique_temp_dir("devcontainer-runtime-mod-test");
        fs::create_dir_all(&root).expect("workspace");
        write_workspace_config(
            &root,
            "{\n  \"image\": \"alpine:3.20\",\n  \"postAttachCommand\": \"echo attach\"\n}\n",
        );
        let engine = root.join("engine");
        write_existing_container_engine(&engine, None);
        let args = existing_container_args(&root, &engine);

        let output = run_user_commands(&args).expect("user command success");

        assert_eq!(output["outcome"], "success");
        assert_eq!(output["command"], "run-user-commands");
        assert_eq!(output["containerId"], "container-id");
        assert_eq!(
            output["remoteWorkspaceFolder"],
            format!(
                "/workspaces/{}",
                root.file_name().expect("workspace name").to_string_lossy()
            )
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn run_exec_reports_context_and_exec_argument_errors() {
        let root = unique_temp_dir("devcontainer-runtime-mod-test");
        fs::create_dir_all(&root).expect("workspace");
        let context_error = run_exec(&[
            "--workspace-folder".to_string(),
            root.display().to_string(),
            "--docker-path".to_string(),
            "/definitely/missing/container-engine".to_string(),
            "/bin/true".to_string(),
        ])
        .expect_err("context error");
        assert!(context_error.contains("Container engine executable not found"));

        write_workspace_config(&root, "{\n  \"image\": \"alpine:3.20\"\n}\n");
        let engine = root.join("engine");
        write_existing_container_engine(&engine, None);
        let mut args = existing_container_args(&root, &engine);
        args.extend([
            "--secrets-file".to_string(),
            root.join("missing-secrets.json").display().to_string(),
            "/bin/true".to_string(),
        ]);

        let exec_error = run_exec(&args).expect_err("exec argument error");
        assert!(exec_error.contains("No such file") || exec_error.contains("not found"));

        let _ = fs::remove_dir_all(root);
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

    fn existing_container_args(workspace: &Path, engine: &Path) -> Vec<String> {
        let mut args = workspace_args(workspace);
        args.extend(docker_args(engine));
        args.extend(["--container-id".to_string(), "container-id".to_string()]);
        args
    }

    fn write_workspace_config(workspace: &Path, contents: &str) -> PathBuf {
        let config_dir = workspace.join(".devcontainer");
        fs::create_dir_all(&config_dir).expect("config dir");
        let config_file = config_dir.join("devcontainer.json");
        fs::write(&config_file, contents).expect("config write");
        fs::canonicalize(&config_file).unwrap_or(config_file)
    }

    fn write_existing_container_engine(engine: &Path, lifecycle_exit: Option<i32>) {
        let lifecycle_exit = lifecycle_exit.unwrap_or(0);
        write_executable_script(
            engine,
            &format!(
                "#!/bin/sh\ncase \"$1\" in\n  ps) printf 'container-id\\n' ;;\n  inspect) printf '[{{\"Config\":{{\"Labels\":{{}},\"Env\":[\"HOME=/home/vscode\"],\"User\":\"vscode\"}},\"Mounts\":[{{\"Destination\":\"/workspace\"}}]}}]\\n' ;;\n  exec)\n    case \"$*\" in\n      *getent\\ passwd*) printf 'vscode:x:1000:1000::/home/vscode:/bin/sh\\n' ;;\n      *) if [ {lifecycle_exit} -ne 0 ]; then printf 'lifecycle failed\\n' >&2; exit {lifecycle_exit}; fi ;;\n    esac\n    ;;\n  *) printf 'unexpected engine command: %s\\n' \"$*\" >&2; exit 2 ;;\nesac\n"
            ),
        );
    }

    fn write_config_mutating_label_engine(
        engine: &Path,
        workspace: &Path,
        config_file: &Path,
        mutate_on_inspect: u32,
    ) {
        let counter = engine.with_extension("inspect-count");
        let workspace = fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
        let workspace = workspace.display();
        let config_file = config_file.display();
        let counter = counter.display();
        write_executable_script(
            engine,
            &format!(
                "#!/bin/sh\nset -eu\ncounter='{counter}'\ncount=0\nif [ -f \"$counter\" ]; then count=$(cat \"$counter\"); fi\ncase \"$1\" in\n  ps) printf 'container-id\\n' ;;\n  inspect)\n    count=$((count + 1))\n    printf '%s' \"$count\" > \"$counter\"\n    printf '[{{\"Config\":{{\"Labels\":{{\"devcontainer.local_folder\":\"{workspace}\"}},\"Env\":[\"HOME=/home/vscode\"],\"User\":\"vscode\"}},\"Mounts\":[{{\"Destination\":\"/workspace\"}}]}}]\\n'\n    if [ \"$count\" -eq {mutate_on_inspect} ]; then printf '{{\"image\":\"alpine:3.20\",\"features\":{{\"./missing-feature\":{{}}}},\"updateRemoteUserUID\":false}}' > '{config_file}'; fi\n    ;;\n  exec)\n    case \"$*\" in\n      *getent\\ passwd*) printf 'vscode:x:1000:1000::/home/vscode:/bin/sh\\n' ;;\n      *) exit 0 ;;\n    esac\n    ;;\n  *) printf 'unexpected engine command: %s\\n' \"$*\" >&2; exit 2 ;;\nesac\n"
            ),
        );
    }

    fn write_labeled_existing_container_engine(
        engine: &Path,
        workspace: &Path,
        config_file: &Path,
    ) {
        let workspace = fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
        let workspace = workspace.display();
        let config_file = config_file.display();
        write_executable_script(
            engine,
            &format!(
                "#!/bin/sh\ncase \"$1\" in\n  ps) printf 'container-id\\n' ;;\n  inspect) printf '[{{\"Config\":{{\"Labels\":{{\"devcontainer.local_folder\":\"{workspace}\",\"devcontainer.config_file\":\"{config_file}\"}},\"Env\":[\"HOME=/home/vscode\"],\"User\":\"vscode\"}},\"Mounts\":[{{\"Destination\":\"/workspace\"}}]}}]\\n' ;;\n  exec)\n    case \"$*\" in\n      *getent\\ passwd*) printf 'vscode:x:1000:1000::/home/vscode:/bin/sh\\n' ;;\n      *) exit 0 ;;\n    esac\n    ;;\n  *) printf 'unexpected engine command: %s\\n' \"$*\" >&2; exit 2 ;;\nesac\n"
            ),
        );
    }
}
