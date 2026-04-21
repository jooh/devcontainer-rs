//! Engine-run argument assembly and engine capability helpers for native containers.

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
            serialized_container_metadata(
                &resolved.configuration,
                remote_workspace_folder,
                common::runtime_options(args).omit_config_remote_env_from_metadata,
            )?
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
    let result = engine::run_engine(
        args,
        vec!["rm".to_string(), "-f".to_string(), container_id.to_string()],
    )?;
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }
    Ok(())
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

    use serde_json::json;

    use crate::runtime::mounts::mount_value_to_engine_arg;

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
}
