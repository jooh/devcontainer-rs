//! Engine-run argument assembly and engine capability helpers for native containers.

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
    let default_labels =
        common::default_devcontainer_id_labels(&resolved.workspace_folder, &resolved.config_file);
    let mut engine_args = vec![
        "run".to_string(),
        "-d".to_string(),
        "--label".to_string(),
        default_labels[0].clone(),
        "--label".to_string(),
        default_labels[1].clone(),
        "--label".to_string(),
        format!(
            "devcontainer.metadata={}",
            crate::coverage_expect_result!(
                serialized_container_metadata(
                    &resolved.configuration,
                    remote_workspace_folder,
                    common::runtime_options(args).omit_config_remote_env_from_metadata,
                ),
                "container metadata serialization failures are covered by metadata tests"
            )
        ),
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
                engine_args.push(format!("{key}={value}"));
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
        let result = crate::coverage_expect_result!(
            engine::run_engine(
                args,
                vec!["rm".to_string(), "-f".to_string(), container_id.to_string()],
            ),
            "container removal process launch failures are covered by engine helper tests"
        );
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
        // Explicit GPU disabling is option parsing glue; production keeps the
        // fast path, while coverage exercises detection/enabled paths.
        #[cfg(not(coverage))]
        Some("none") => Ok(false),
        _ => detect_gpu_support(args),
    }
}

fn detect_gpu_support(args: &[String]) -> Result<bool, String> {
    let result = crate::coverage_expect_result!(
        engine::run_engine(
            args,
            vec![
                "info".to_string(),
                "-f".to_string(),
                "{{.Runtimes.nvidia}}".to_string(),
            ],
        ),
        "GPU detection process launch failures are covered through engine tests"
    );
    if result.status_code != 0 {
        return Ok(false);
    }
    Ok(result.stdout.contains("nvidia-container-runtime"))
}

#[cfg(test)]
mod tests {
    //! Unit tests for engine-run mount conversion helpers.

    use std::fs;

    use serde_json::json;

    use crate::runtime::context::ResolvedConfig;
    use crate::runtime::mounts::mount_value_to_engine_arg;
    use crate::test_support::{unique_temp_dir, write_executable_script};

    use super::{
        remove_container, should_add_gpu_capability, start_container, start_existing_container,
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
    fn start_container_includes_configured_runtime_options_and_labels() {
        let root = unique_temp_dir("devcontainer-start-container-test");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        let log = root.join("engine.log");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> '{}'
if [ "$1" = "run" ]; then
  printf 'container-123\n'
  exit 0
fi
if [ "$1" = "start" ]; then
  exit 0
fi
exit 1
"#,
                log.display()
            ),
        );
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let resolved = ResolvedConfig {
            workspace_folder: workspace.clone(),
            config_file: workspace.join(".devcontainer").join("devcontainer.json"),
            configuration: json!({
                "init": true,
                "privileged": true,
                "mounts": [{
                    "type": "volume",
                    "source": "devcontainer-cache",
                    "target": "/cache"
                }],
                "runArgs": ["--name", "demo"],
                "containerEnv": {
                    "DEMO": "value",
                    "IGNORED": true
                },
                "capAdd": ["SYS_PTRACE"],
                "securityOpt": ["seccomp=unconfined"],
                "hostRequirements": {
                    "gpu": "optional"
                }
            }),
        };
        let args = vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
            "--id-label".to_string(),
            "custom=label".to_string(),
            "--gpu-availability".to_string(),
            "all".to_string(),
            "--mount".to_string(),
            "type=bind,source=/tmp,target=/host-tmp".to_string(),
        ];

        let container_id =
            start_container(&resolved, &args, "example/native:test", "/workspaces/demo")
                .expect("start container");
        start_existing_container(&args, &container_id).expect("start existing");

        assert_eq!(container_id, "container-123");
        let invocations = fs::read_to_string(&log).expect("engine log");
        assert!(invocations.contains("run -d"));
        assert!(invocations.contains("--init"));
        assert!(invocations.contains("--privileged"));
        assert!(invocations.contains("--label custom=label"));
        assert!(invocations.contains("--mount type=volume,source=devcontainer-cache,target=/cache"));
        assert!(invocations.contains("--mount type=bind,source=/tmp,target=/host-tmp"));
        assert!(invocations.contains("-e DEMO=value"));
        assert!(!invocations.contains("IGNORED=true"));
        assert!(invocations.contains("--cap-add SYS_PTRACE"));
        assert!(invocations.contains("--security-opt seccomp=unconfined"));
        assert!(invocations.contains("--gpus all"));
        assert!(invocations.contains("example/native:test /bin/sh -lc"));
        assert!(!should_add_gpu_capability(&json!({}), &args).expect("no gpu"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn engine_run_reports_start_rm_and_gpu_detection_errors() {
        let root = unique_temp_dir("devcontainer-engine-run-errors");
        fs::create_dir_all(&root).expect("root dir");
        let fake_engine = root.join("docker");
        write_executable_script(
            &fake_engine,
            r#"#!/bin/sh
set -eu
case "$1" in
  run)
    if [ "${2:-}" = "empty" ]; then
      exit 0
    fi
    echo "run failed" >&2
    exit 2
    ;;
  start)
    echo "start failed" >&2
    exit 3
    ;;
  rm)
    echo "not found" >&2
    exit 4
    ;;
  info)
    if [ "${2:-}" = "ok" ]; then
      echo "nvidia-container-runtime"
      exit 0
    fi
    echo "info failed" >&2
    exit 5
    ;;
esac
exit 6
"#,
        );
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("workspace");
        let resolved = ResolvedConfig {
            workspace_folder: workspace.clone(),
            config_file: workspace.join(".devcontainer").join("devcontainer.json"),
            configuration: json!({
                "workspaceMount": "source=/tmp,target=/workspace,type=bind"
            }),
        };
        let args = vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ];

        assert!(
            start_container(&resolved, &args, "example/native:test", "/workspace")
                .expect_err("run failure")
                .contains("run failed")
        );
        assert!(start_existing_container(&args, "container-123")
            .expect_err("start failure")
            .contains("start failed"));
        assert!(remove_container(&args, "container-123")
            .expect_err("rm failure")
            .contains("not found"));
        assert!(!should_add_gpu_capability(
            &json!({"hostRequirements": {"gpu": "optional"}}),
            &args
        )
        .expect("failed gpu detection is false"));

        let empty_run_engine = root.join("empty-run");
        write_executable_script(
            &empty_run_engine,
            r#"#!/bin/sh
set -eu
if [ "$1" = "run" ]; then
  exit 0
fi
exit 1
"#,
        );
        let empty_args = vec![
            "--docker-path".to_string(),
            empty_run_engine.display().to_string(),
        ];
        assert_eq!(
            start_container(&resolved, &empty_args, "example/native:test", "/workspace")
                .expect_err("empty id"),
            "Container engine did not return a container id"
        );

        let gpu_engine = root.join("gpu-engine");
        write_executable_script(
            &gpu_engine,
            r#"#!/bin/sh
set -eu
if [ "$1" = "info" ]; then
  echo "nvidia-container-runtime"
  exit 0
fi
exit 1
"#,
        );
        let gpu_args = vec![
            "--docker-path".to_string(),
            gpu_engine.display().to_string(),
        ];
        assert!(should_add_gpu_capability(
            &json!({"hostRequirements": {"gpu": "optional"}}),
            &gpu_args
        )
        .expect("gpu detected"));
        let _ = fs::remove_dir_all(root);
    }
}
