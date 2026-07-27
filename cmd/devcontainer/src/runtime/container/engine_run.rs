//! Engine-run argument assembly and engine capability helpers for native containers.

use std::collections::HashMap;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::commands::common;

use super::super::context::{
    additional_mounts_for_workspace_target, workspace_mount_for_args, ResolvedConfig,
};
use super::super::engine;
use super::super::metadata::serialized_container_metadata;
use super::super::mounts::mount_value_to_engine_arg;

pub(super) fn start_container(
    resolved: &ResolvedConfig,
    args: &[String],
    image_name: &str,
    remote_workspace_folder: &str,
) -> Result<String, String> {
    let omit_config_remote_env = common::runtime_options(args).omit_config_remote_env_from_metadata;
    let metadata = serialized_container_metadata(
        &resolved.configuration,
        remote_workspace_folder,
        omit_config_remote_env,
    );
    start_container_with_metadata(
        resolved,
        args,
        image_name,
        remote_workspace_folder,
        metadata,
    )
}

fn start_container_with_metadata(
    resolved: &ResolvedConfig,
    args: &[String],
    image_name: &str,
    remote_workspace_folder: &str,
    metadata: Result<String, String>,
) -> Result<String, String> {
    let image_environment = resolved
        .configuration
        .get("containerEnv")
        .and_then(Value::as_object)
        .filter(|environment| {
            environment
                .values()
                .filter_map(Value::as_str)
                .any(contains_environment_reference)
        })
        .map(|_| inspect_image_environment(args, image_name))
        .transpose()?
        .unwrap_or_default();
    let default_labels =
        common::default_devcontainer_id_labels(&resolved.workspace_folder, &resolved.config_file);
    let metadata = metadata?;
    let mut engine_args = vec![
        "run".to_string(),
        "-d".to_string(),
        "--label".to_string(),
        default_labels[0].clone(),
        "--label".to_string(),
        default_labels[1].clone(),
        "--label".to_string(),
        format!("devcontainer.metadata={metadata}"),
        "--mount".to_string(),
        workspace_mount_for_args(resolved, remote_workspace_folder, args),
    ];
    if resolved.configuration.get("workspaceMount").is_none() {
        for mount in additional_mounts_for_workspace_target(resolved, remote_workspace_folder, args)
        {
            engine_args.push("--mount".to_string());
            engine_args.push(mount);
        }
    }
    if resolved
        .configuration
        .get("init")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        engine_args.push("--init".to_string());
    }
    if resolved
        .configuration
        .get("privileged")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        engine_args.push("--privileged".to_string());
    }
    for label in common::parse_option_values(args, "--id-label") {
        engine_args.push("--label".to_string());
        engine_args.push(label);
    }
    if let Some(mounts) = resolved
        .configuration
        .get("mounts")
        .and_then(Value::as_array)
    {
        for mount in mounts.iter().filter_map(mount_value_to_engine_arg) {
            engine_args.push("--mount".to_string());
            engine_args.push(mount);
        }
    }
    for mount in crate::runtime::mounts::cli_mount_values(args)? {
        engine_args.push("--mount".to_string());
        engine_args.push(mount);
    }
    if let Some(run_args) = resolved
        .configuration
        .get("runArgs")
        .and_then(Value::as_array)
    {
        for arg in run_args.iter().filter_map(Value::as_str) {
            engine_args.push(arg.to_string());
        }
    }
    if let Some(container_env) = resolved
        .configuration
        .get("containerEnv")
        .and_then(Value::as_object)
    {
        for (key, value) in container_env {
            if let Some(value) = value.as_str() {
                engine_args.push("-e".to_string());
                engine_args.push(format!(
                    "{key}={}",
                    expand_environment_references(value, &image_environment)
                ));
            }
        }
    }
    if let Some(cap_add) = resolved
        .configuration
        .get("capAdd")
        .and_then(Value::as_array)
    {
        for capability in cap_add.iter().filter_map(Value::as_str) {
            engine_args.push("--cap-add".to_string());
            engine_args.push(capability.to_string());
        }
    }
    if let Some(security_opt) = resolved
        .configuration
        .get("securityOpt")
        .and_then(Value::as_array)
    {
        for option in security_opt.iter().filter_map(Value::as_str) {
            engine_args.push("--security-opt".to_string());
            engine_args.push(option.to_string());
        }
    }
    if should_add_gpu_capability(&resolved.configuration, args)? {
        engine_args.push("--gpus".to_string());
        engine_args.push("all".to_string());
    }
    engine_args.push(image_name.to_string());
    engine_args.push("/bin/sh".to_string());
    engine_args.push("-lc".to_string());
    engine_args.push("while sleep 1000; do :; done".to_string());

    let result = engine::run_engine(args, engine_args)?;
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }

    let container_id = result.stdout.trim().to_string();
    if container_id.is_empty() {
        return Err("Container engine did not return a container id".to_string());
    }

    Ok(container_id)
}

fn inspect_image_environment(
    args: &[String],
    image_name: &str,
) -> Result<HashMap<String, String>, String> {
    let result = engine::run_engine(
        args,
        vec![
            "image".to_string(),
            "inspect".to_string(),
            "--format".to_string(),
            "{{json .Config.Env}}".to_string(),
            image_name.to_string(),
        ],
    )?;
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }
    let entries: Option<Vec<String>> = serde_json::from_str(result.stdout.trim())
        .map_err(|error| format!("Failed to parse environment from image {image_name}: {error}"))?;
    Ok(entries
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            entry
                .split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
        })
        .collect())
}

fn contains_environment_reference(value: &str) -> bool {
    value.as_bytes().windows(2).any(|window| {
        window[0] == b'$'
            && (window[1] == b'{' || window[1].is_ascii_alphabetic() || window[1] == b'_')
    })
}

fn expand_environment_references(value: &str, environment: &HashMap<String, String>) -> String {
    let bytes = value.as_bytes();
    let mut expanded = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' || index + 1 == bytes.len() {
            let character = value[index..].chars().next().expect("non-empty remainder");
            expanded.push(character);
            index += character.len_utf8();
            continue;
        }
        let (name_start, name_end, reference_end) = if bytes[index + 1] == b'{' {
            let Some(closing) = bytes[index + 2..].iter().position(|byte| *byte == b'}') else {
                expanded.push('$');
                index += 1;
                continue;
            };
            let name_end = index + 2 + closing;
            (index + 2, name_end, name_end + 1)
        } else {
            let mut end = index + 1;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            (index + 1, end, end)
        };
        let name = &value[name_start..name_end];
        if let Some(replacement) = environment.get(name) {
            expanded.push_str(replacement);
        } else {
            expanded.push_str(&value[index..reference_end]);
        }
        index = reference_end;
    }
    expanded
}

pub(super) fn start_existing_container(args: &[String], container_id: &str) -> Result<(), String> {
    let result = engine::run_engine(args, vec!["start".to_string(), container_id.to_string()])?;
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }
    Ok(())
}

pub(super) fn remove_container(args: &[String], container_id: &str) -> Result<(), String> {
    let mut attempt = 0;
    loop {
        let result = engine::run_engine(
            args,
            vec!["rm".to_string(), "-f".to_string(), container_id.to_string()],
        )?;
        if result.status_code == 0 {
            return Ok(());
        }

        let error = engine::stderr_or_stdout(&result);
        if attempt == 6 || !container_removal_already_in_progress(&error) {
            return Err(error);
        }
        attempt += 1;
        thread::sleep(Duration::from_millis(100));
    }
}

fn container_removal_already_in_progress(error: &str) -> bool {
    error.to_ascii_lowercase().contains("already in progress")
}

pub(crate) fn should_add_gpu_capability(
    configuration: &Value,
    args: &[String],
) -> Result<bool, String> {
    if configuration
        .get("hostRequirements")
        .and_then(|requirements| requirements.get("gpu"))
        .is_none()
    {
        return Ok(false);
    }

    match common::runtime_options(args).gpu_availability.as_deref() {
        Some("all") => Ok(true),
        Some("none") => Ok(false),
        _ => detect_gpu_support(args),
    }
}

fn detect_gpu_support(args: &[String]) -> Result<bool, String> {
    let result = engine::run_engine(
        args,
        vec![
            "info".to_string(),
            "-f".to_string(),
            "{{.Runtimes.nvidia}}".to_string(),
        ],
    )?;
    if result.status_code != 0 {
        return Ok(false);
    }
    Ok(result.stdout.contains("nvidia-container-runtime"))
}

#[cfg(test)]
mod tests {
    //! Unit tests for engine-run mount conversion helpers.

    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;

    use serde_json::json;

    use crate::commands::common::{test_env_defaults, DEVCONTAINER_GPU_AVAILABILITY};
    use crate::runtime::context::ResolvedConfig;
    use crate::runtime::mounts::mount_value_to_engine_arg;
    use crate::test_support::{unique_temp_dir, write_executable_script};

    use super::{
        contains_environment_reference, expand_environment_references, inspect_image_environment,
        remove_container, should_add_gpu_capability, start_container,
        start_container_with_metadata, start_existing_container,
    };

    #[test]
    fn mount_argument_preserves_read_only_and_alias_keys() {
        let mount = mount_value_to_engine_arg(&json!({
            "type": "bind",
            "src": "/cache",
            "dst": "/workspace/cache",
            "readOnly": true,
        }))
        .expect("mount argument");

        assert_eq!(
            mount,
            "type=bind,source=/cache,target=/workspace/cache,readonly"
        );
    }

    #[test]
    fn mount_argument_preserves_additional_scalar_options() {
        let mount = mount_value_to_engine_arg(&json!({
            "type": "volume",
            "source": "devcontainer-cache",
            "target": "/cache",
            "external": true,
            "consistency": "delegated",
        }))
        .expect("mount argument");

        assert_eq!(
            mount,
            "type=volume,source=devcontainer-cache,target=/cache,consistency=delegated,external=true"
        );
    }

    #[test]
    fn remove_container_retries_concurrent_removal_errors() {
        let root = unique_temp_dir("devcontainer-remove-container-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        let attempts = root.join("rm-attempts");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
attempts="{attempts}"
current=0
if [ -f "$attempts" ]; then
  current="$(cat "$attempts")"
fi
next=$((current + 1))
printf '%s\n' "$next" > "$attempts"
if [ "$1" = "rm" ] && [ "$next" -lt 3 ]; then
  echo "Error: removal of container fake-container is already in progress" >&2
  exit 1
fi
exit 0
"#,
                attempts = attempts.display()
            ),
        );
        let args = vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ];

        remove_container(&args, "fake-container").expect("container removal");

        assert_eq!(
            fs::read_to_string(&attempts).expect("attempts file").trim(),
            "3"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn start_container_includes_run_args_and_returns_container_id() {
        let root = unique_temp_dir("devcontainer-start-container-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        let invocation_log = root.join("invocations.log");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
printf '%s\n' "$*" > "{invocation_log}"
case "$1" in
  run)
    printf 'created-container\n'
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                invocation_log = invocation_log.display()
            ),
        );
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("workspace dir");
        let resolved = resolved_config(
            &workspace,
            json!({
                "workspaceMount": "source=/workspace,target=/workspace,type=bind",
                "runArgs": ["--name", "devcontainer-test"],
                "containerEnv": {
                    "EDITOR": "vim"
                },
                "capAdd": ["SYS_PTRACE"],
                "securityOpt": ["seccomp=unconfined"]
            }),
        );

        let container_id = start_container(
            &resolved,
            &engine_args(&fake_engine),
            "alpine:3.20",
            "/workspace",
        )
        .expect("container should start");

        assert_eq!(container_id, "created-container");
        let invocation = fs::read_to_string(&invocation_log).expect("invocation log");
        assert!(invocation.contains("--name devcontainer-test"));
        assert!(invocation.contains("-e EDITOR=vim"));
        assert!(invocation.contains("--cap-add SYS_PTRACE"));
        assert!(invocation.contains("--security-opt seccomp=unconfined"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn start_container_expands_container_env_from_the_built_image() {
        let root = unique_temp_dir("devcontainer-start-container-expanded-env-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        let invocation_log = root.join("invocations.log");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "{invocation_log}"
case "$1 $2" in
  "image inspect") printf '["PATH=/usr/local/bin:/usr/bin","HOME=/root"]\n' ;;
  "run -d") printf 'created-container\n' ;;
  *) echo "unexpected command $*" >&2; exit 2 ;;
esac
"#,
                invocation_log = invocation_log.display()
            ),
        );
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("workspace dir");
        let resolved = resolved_config(
            &workspace,
            json!({ "containerEnv": {
                "PATH": "/usr/local/share/codespace-shims:${PATH}"
            } }),
        );

        start_container(
            &resolved,
            &engine_args(&fake_engine),
            "example/devcontainer:features",
            "/workspace",
        )
        .expect("container should start");

        let invocation = fs::read_to_string(&invocation_log).expect("invocation log");
        assert!(invocation
            .contains("image inspect --format {{json .Config.Env}} example/devcontainer:features"));
        assert!(
            invocation.contains("-e PATH=/usr/local/share/codespace-shims:/usr/local/bin:/usr/bin")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn environment_expansion_supports_braced_and_plain_references() {
        let environment = HashMap::from([
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("HOME".to_string(), "/root".to_string()),
        ]);

        assert_eq!(
            expand_environment_references("λ:${PATH}:$HOME:$UNKNOWN", &environment),
            "λ:/usr/bin:/root:$UNKNOWN"
        );
        assert_eq!(
            expand_environment_references("${UNKNOWN}", &environment),
            "${UNKNOWN}"
        );
        assert_eq!(
            expand_environment_references("unterminated-${PATH", &environment),
            "unterminated-${PATH"
        );
        assert_eq!(
            expand_environment_references("trailing-$", &environment),
            "trailing-$"
        );
        assert!(contains_environment_reference("$HOME"));
        assert!(contains_environment_reference("${PATH}"));
        assert!(!contains_environment_reference("cost-$5"));
    }

    #[test]
    fn inspect_image_environment_reports_engine_and_output_errors() {
        let root = unique_temp_dir("devcontainer-inspect-image-environment-errors-test");
        fs::create_dir_all(&root).expect("root dir");
        let failing_engine = root.join("failing-docker");
        write_executable_script(
            &failing_engine,
            "#!/bin/sh\necho 'image inspection failed' >&2\nexit 7\n",
        );

        let error = inspect_image_environment(&engine_args(&failing_engine), "example/image")
            .expect_err("nonzero image inspection should fail");
        assert_eq!(error, "image inspection failed");

        let missing_error =
            inspect_image_environment(&engine_args(&root.join("missing-docker")), "example/image")
                .expect_err("missing engine should fail");
        assert!(missing_error.contains("Container engine executable not found"));

        let invalid_engine = root.join("invalid-output-docker");
        write_executable_script(&invalid_engine, "#!/bin/sh\nprintf 'not-json\\n'\n");
        let invalid_error =
            inspect_image_environment(&engine_args(&invalid_engine), "example/image")
                .expect_err("invalid image environment should fail");
        assert!(invalid_error.contains("Failed to parse environment from image example/image"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn start_container_includes_runtime_flags_mounts_gpu_and_git_common_dir() {
        let root = unique_temp_dir("devcontainer-start-container-flags-test");
        let workspace = root.join("worktree");
        fs::create_dir_all(&workspace).expect("workspace dir");
        fs::write(
            workspace.join(".git"),
            "gitdir: ../repo/.git/worktrees/worktree\n",
        )
        .expect("git file");
        let fake_engine = root.join("docker");
        let invocation_log = root.join("invocations.log");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "{invocation_log}"
case "$1" in
  info)
    printf 'nvidia-container-runtime\n'
    ;;
  run)
    printf 'created-container\n'
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
                invocation_log = invocation_log.display()
            ),
        );
        let resolved = resolved_config(
            &workspace,
            json!({
                "workspaceFolder": "/workspace",
                "init": true,
                "privileged": true,
                "mounts": [
                    "type=bind,source=/cache,target=/cache"
                ],
                "hostRequirements": {
                    "gpu": "optional"
                }
            }),
        );
        let mut args = engine_args(&fake_engine);
        args.extend([
            "--mount-git-worktree-common-dir".to_string(),
            "true".to_string(),
            "--mount".to_string(),
            "type=volume,target=/cli-cache".to_string(),
        ]);

        let container_id = start_container(&resolved, &args, "alpine:3.20", "/workspace")
            .expect("container should start");

        assert_eq!(container_id, "created-container");
        let invocation = fs::read_to_string(&invocation_log).expect("invocation log");
        assert!(invocation.contains("info -f {{.Runtimes.nvidia}}"));
        assert!(invocation.contains("--mount type=bind,source=/cache,target=/cache"));
        assert!(invocation.contains("--mount type=volume,target=/cli-cache"));
        assert!(invocation.contains("--mount type=bind,source="));
        assert!(invocation.contains("target=/repo/.git"));
        assert!(invocation.contains("--init"));
        assert!(invocation.contains("--privileged"));
        assert!(invocation.contains("--gpus all"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn start_container_reports_engine_failures_and_empty_ids() {
        let root = unique_temp_dir("devcontainer-start-container-errors-test");
        fs::create_dir_all(&root).expect("root dir");
        let failing_engine = root.join("failing-docker");
        write_executable_script(
            &failing_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  run)
    echo "run failed" >&2
    exit 2
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("workspace dir");
        let resolved = resolved_config(
            &workspace,
            json!({
                "workspaceMount": "source=/workspace,target=/workspace,type=bind"
            }),
        );

        let run_error = start_container(
            &resolved,
            &engine_args(&failing_engine),
            "alpine:3.20",
            "/workspace",
        )
        .expect_err("run failure should propagate");
        assert_eq!(run_error, "run failed");

        let empty_id_engine = root.join("empty-id-docker");
        write_executable_script(
            &empty_id_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  run)
    exit 0
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );

        let empty_id_error = start_container(
            &resolved,
            &engine_args(&empty_id_engine),
            "alpine:3.20",
            "/workspace",
        )
        .expect_err("empty id should fail");
        assert_eq!(
            empty_id_error,
            "Container engine did not return a container id"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn start_container_reports_metadata_serialization_errors() {
        let root = unique_temp_dir("devcontainer-start-container-metadata-error-test");
        fs::create_dir_all(&root).expect("root dir");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("workspace dir");
        let resolved = resolved_config(
            &workspace,
            json!({
                "workspaceMount": "source=/workspace,target=/workspace,type=bind"
            }),
        );

        let error = start_container_with_metadata(
            &resolved,
            &[],
            "alpine:3.20",
            "/workspace",
            Err("metadata failed".to_string()),
        )
        .expect_err("metadata error should propagate");

        assert_eq!(error, "metadata failed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn start_existing_container_reports_engine_status_failures() {
        let root = unique_temp_dir("devcontainer-start-existing-error-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  start)
    echo "start failed" >&2
    exit 2
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );

        let error = start_existing_container(&engine_args(&fake_engine), "existing-container")
            .expect_err("start failure should propagate");

        assert_eq!(error, "start failed");

        let success_engine = root.join("success-docker");
        write_executable_script(
            &success_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  start)
    exit 0
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        start_existing_container(&engine_args(&success_engine), "existing-container")
            .expect("successful start should be accepted");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remove_container_reports_spawn_and_non_retryable_errors() {
        let missing_error = remove_container(
            &[
                "--docker-path".to_string(),
                "/path/that/does/not/exist".to_string(),
            ],
            "missing-container",
        )
        .expect_err("missing engine should fail");
        assert!(missing_error.contains("Container engine executable not found"));

        let root = unique_temp_dir("devcontainer-remove-container-error-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  rm)
    echo "permission denied" >&2
    exit 1
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );

        let status_error = remove_container(&engine_args(&fake_engine), "blocked-container")
            .expect_err("non retryable rm failure should propagate");

        assert_eq!(status_error, "permission denied");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn gpu_capability_respects_options_and_detection_results() {
        let gpu_config = json!({
            "hostRequirements": {
                "gpu": "optional"
            }
        });
        assert!(!should_add_gpu_capability(&json!({}), &[]).expect("no gpu requirement"));
        assert!(should_add_gpu_capability(
            &gpu_config,
            &["--gpu-availability".to_string(), "all".to_string()],
        )
        .expect("explicit all"));
        assert!(!should_add_gpu_capability(
            &gpu_config,
            &["--gpu-availability".to_string(), "none".to_string()],
        )
        .expect("explicit none"));
        let env = test_env_defaults(&[(DEVCONTAINER_GPU_AVAILABILITY, "all")]);
        assert!(should_add_gpu_capability(&gpu_config, &[]).expect("env all"));
        assert!(!should_add_gpu_capability(
            &gpu_config,
            &["--gpu-availability".to_string(), "none".to_string()],
        )
        .expect("cli none overrides env all"));
        drop(env);
        let env = test_env_defaults(&[(DEVCONTAINER_GPU_AVAILABILITY, "none")]);
        assert!(!should_add_gpu_capability(&gpu_config, &[]).expect("env none"));
        drop(env);

        let root = unique_temp_dir("devcontainer-gpu-detection-test");
        fs::create_dir_all(&root).expect("root dir");
        let gpu_engine = root.join("gpu-docker");
        write_executable_script(
            &gpu_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  info)
    echo "nvidia-container-runtime"
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        assert!(
            should_add_gpu_capability(&gpu_config, &engine_args(&gpu_engine))
                .expect("gpu runtime should be detected")
        );

        let no_gpu_engine = root.join("no-gpu-docker");
        write_executable_script(
            &no_gpu_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  info)
    echo "<no value>"
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        assert!(
            !should_add_gpu_capability(&gpu_config, &engine_args(&no_gpu_engine))
                .expect("missing gpu runtime should be false")
        );

        let failing_gpu_engine = root.join("failing-gpu-docker");
        write_executable_script(
            &failing_gpu_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  info)
    echo "info failed" >&2
    exit 1
    ;;
  *)
    echo "unexpected command $1" >&2
    exit 2
    ;;
esac
"#,
        );
        assert!(
            !should_add_gpu_capability(&gpu_config, &engine_args(&failing_gpu_engine))
                .expect("status failure should disable gpu")
        );

        let missing_error = should_add_gpu_capability(
            &gpu_config,
            &[
                "--docker-path".to_string(),
                "/path/that/does/not/exist".to_string(),
            ],
        )
        .expect_err("spawn failure should propagate");
        assert!(missing_error.contains("Container engine executable not found"));
        let _ = fs::remove_dir_all(root);
    }

    fn engine_args(fake_engine: &Path) -> Vec<String> {
        vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ]
    }

    fn resolved_config(
        workspace_folder: &Path,
        configuration: serde_json::Value,
    ) -> ResolvedConfig {
        ResolvedConfig {
            workspace_folder: workspace_folder.to_path_buf(),
            config_file: workspace_folder
                .join(".devcontainer")
                .join("devcontainer.json"),
            configuration,
        }
    }
}
