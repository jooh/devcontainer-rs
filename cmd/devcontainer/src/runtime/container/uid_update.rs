//! UID-update image preparation and image-inspection helpers for native runtime flows.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::commands::common;

use super::super::compose;
use super::super::context::ResolvedConfig;
use super::super::engine;
use super::super::paths::unique_temp_path;

const UID_UPDATE_IMAGE_INSPECT_FORMAT: &str =
    "{{.Config.User}}\n{{.Os}}/{{.Architecture}}{{if .Variant}}/{{.Variant}}{{end}}";
const UID_UPDATE_IMAGE_INSPECT_FORMAT_NO_VARIANT: &str =
    "{{.Config.User}}\n{{.Os}}/{{.Architecture}}";
const UID_UPDATE_DOCKERFILE: &str =
    include_str!("../../../../../upstream/scripts/updateUID.Dockerfile");

#[derive(Debug, Eq, PartialEq)]
struct UidUpdateDetails {
    remote_user: String,
    image_user: String,
    updated_image_name: String,
    platform: Option<String>,
}

#[derive(Debug, Eq, PartialEq)]
struct ImageInspectDetails {
    user: String,
    platform: Option<String>,
}

pub(crate) fn prepare_up_image(
    resolved: &ResolvedConfig,
    args: &[String],
    image_name: &str,
) -> Result<String, String> {
    prepare_up_image_for_platform(
        resolved,
        args,
        image_name,
        is_uid_update_platform_supported(),
    )
}

fn prepare_up_image_for_platform(
    resolved: &ResolvedConfig,
    args: &[String],
    image_name: &str,
    platform_supported: bool,
) -> Result<String, String> {
    if !should_update_remote_user_uid(&resolved.configuration, args, platform_supported) {
        return Ok(image_name.to_string());
    }

    let compose_service_user = if compose::uses_compose_config(&resolved.configuration) {
        compose::load_compose_spec(resolved)?.and_then(|spec| spec.user)
    } else {
        None
    };
    let Some(update) = resolve_uid_update_details(
        &resolved.configuration,
        args,
        &resolved.workspace_folder,
        image_name,
        compose_service_user.as_deref(),
    )?
    else {
        return Ok(image_name.to_string());
    };
    let (new_uid, new_gid) = host_uid_gid()?;
    let build_context = unique_uid_update_build_context();
    fs::create_dir_all(&build_context).map_err(|error| error.to_string())?;
    let dockerfile = build_context.join("updateUID.Dockerfile");
    fs::write(&dockerfile, UID_UPDATE_DOCKERFILE).map_err(|error| error.to_string())?;
    let mut build_args = vec!["build".to_string()];
    if let Some(platform) = &update.platform {
        build_args.push("--platform".to_string());
        build_args.push(platform.clone());
    }
    build_args.extend([
        "--build-arg".to_string(),
        format!("BASE_IMAGE={}", uid_update_base_image(args, image_name)),
        "--build-arg".to_string(),
        format!("REMOTE_USER={}", update.remote_user),
        "--build-arg".to_string(),
        format!("NEW_UID={new_uid}"),
        "--build-arg".to_string(),
        format!("NEW_GID={new_gid}"),
        "--build-arg".to_string(),
        format!("IMAGE_USER={}", update.image_user),
        "-t".to_string(),
        update.updated_image_name.clone(),
        "-f".to_string(),
        dockerfile.display().to_string(),
        build_context.display().to_string(),
    ]);

    let result = engine::run_engine(args, std::mem::take(&mut build_args))?;
    let _ = fs::remove_dir_all(&build_context);
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }

    Ok(update.updated_image_name)
}

pub(crate) fn should_update_remote_user_uid(
    configuration: &Value,
    args: &[String],
    platform_supported: bool,
) -> bool {
    if !platform_supported {
        return false;
    }

    let default_value = common::runtime_options(args)
        .update_remote_user_uid_default
        .unwrap_or_else(|| "on".to_string());
    if default_value == "never" {
        return false;
    }

    let should_update = configuration
        .get("updateRemoteUserUID")
        .and_then(Value::as_bool)
        .unwrap_or(default_value == "on");
    if !should_update {
        return false;
    }

    configuration.is_object()
}

fn uid_update_details(
    configuration: &Value,
    workspace_folder: &Path,
    image_name: &str,
    image_user: &str,
    runtime_user: Option<&str>,
    platform: Option<&str>,
) -> Option<UidUpdateDetails> {
    let remote_user = uid_update_remote_user(configuration, runtime_user, image_user)?;
    Some(UidUpdateDetails {
        remote_user,
        image_user: image_user.to_string(),
        updated_image_name: uid_update_image_name(workspace_folder, image_name),
        platform: platform.map(str::to_string),
    })
}

fn resolve_uid_update_details(
    configuration: &Value,
    args: &[String],
    workspace_folder: &Path,
    image_name: &str,
    compose_service_user: Option<&str>,
) -> Result<Option<UidUpdateDetails>, String> {
    let runtime_user = uid_update_run_args_user(configuration)
        .or_else(|| compose_service_user.map(str::to_string));
    if let Some(user) = uid_update_configured_user(configuration, runtime_user.as_deref()) {
        if !is_updatable_user(&user) {
            return Ok(None);
        }
    }

    let Some(image_details) =
        inspect_image_details_for_uid_update(args, configuration, image_name)?
    else {
        return Ok(None);
    };

    Ok(uid_update_details(
        configuration,
        workspace_folder,
        image_name,
        &image_details.user,
        runtime_user.as_deref(),
        image_details.platform.as_deref(),
    ))
}

fn uid_update_remote_user(
    configuration: &Value,
    run_args_user: Option<&str>,
    image_user: &str,
) -> Option<String> {
    let user = configuration
        .get("remoteUser")
        .or_else(|| configuration.get("containerUser"))
        .and_then(Value::as_str)
        .or(run_args_user)
        .unwrap_or(image_user);
    is_updatable_user(user).then(|| user.to_string())
}

fn uid_update_configured_user(
    configuration: &Value,
    run_args_user: Option<&str>,
) -> Option<String> {
    configuration
        .get("remoteUser")
        .or_else(|| configuration.get("containerUser"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| run_args_user.map(str::to_string))
}

fn uid_update_run_args_user(configuration: &Value) -> Option<String> {
    let run_args = configuration.get("runArgs").and_then(Value::as_array)?;
    for index in (0..run_args.len()).rev() {
        let Some(arg) = run_args[index].as_str() else {
            continue;
        };
        if matches!(arg, "-u" | "--user") {
            if let Some(user) = run_args.get(index + 1).and_then(Value::as_str) {
                return Some(user.to_string());
            }
            continue;
        }
        if let Some(user) = arg
            .strip_prefix("-u=")
            .or_else(|| arg.strip_prefix("--user="))
        {
            return Some(user.to_string());
        }
    }
    None
}

fn is_updatable_user(user: &str) -> bool {
    user != "root" && !user.chars().all(|character| character.is_ascii_digit())
}

fn inspect_image_details_for_uid_update(
    args: &[String],
    configuration: &Value,
    image_name: &str,
) -> Result<Option<ImageInspectDetails>, String> {
    match inspect_image_details_for_uid_update_once(args, image_name)? {
        Some(details) => Ok(Some(details)),
        None if configuration.get("image").and_then(Value::as_str).is_some() => {
            pull_image_for_uid_update(args, image_name)?;
            inspect_image_details_for_uid_update_once(args, image_name)
        }
        None => Ok(None),
    }
}

fn inspect_image_details_for_uid_update_once(
    args: &[String],
    image_name: &str,
) -> Result<Option<ImageInspectDetails>, String> {
    let result = engine::run_engine(
        args,
        vec![
            "image".to_string(),
            "inspect".to_string(),
            "--format".to_string(),
            UID_UPDATE_IMAGE_INSPECT_FORMAT.to_string(),
            image_name.to_string(),
        ],
    )?;
    if result.status_code != 0 {
        let error = engine::stderr_or_stdout(&result);
        if is_missing_variant_template_error(&error) {
            return inspect_image_details_without_variant(args, image_name);
        }
        if is_missing_local_image_inspect_error(&error) {
            return Ok(None);
        }
        return Err(error);
    }

    parse_image_inspect_details(&result.stdout)
}

fn inspect_image_details_without_variant(
    args: &[String],
    image_name: &str,
) -> Result<Option<ImageInspectDetails>, String> {
    let result = engine::run_engine(
        args,
        vec![
            "image".to_string(),
            "inspect".to_string(),
            "--format".to_string(),
            UID_UPDATE_IMAGE_INSPECT_FORMAT_NO_VARIANT.to_string(),
            image_name.to_string(),
        ],
    )?;
    if result.status_code != 0 {
        let error = engine::stderr_or_stdout(&result);
        if is_missing_local_image_inspect_error(&error) {
            return Ok(None);
        }
        return Err(error);
    }
    parse_image_inspect_details(&result.stdout)
}

fn parse_image_inspect_details(stdout: &str) -> Result<Option<ImageInspectDetails>, String> {
    let mut lines = stdout.lines();
    let user = lines
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("root")
        .to_string();
    let platform = lines
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    Ok(Some(ImageInspectDetails { user, platform }))
}

fn pull_image_for_uid_update(args: &[String], image_name: &str) -> Result<(), String> {
    let result = engine::run_engine(args, vec!["pull".to_string(), image_name.to_string()])?;
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }
    Ok(())
}

fn is_missing_local_image_inspect_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("no such image") || error.contains("image not known")
}

fn is_missing_variant_template_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("can't evaluate field variant")
}

fn uid_update_image_name(workspace_folder: &Path, image_name: &str) -> String {
    let local_image_name = uid_update_local_image_name(workspace_folder);
    let base_image_name = if image_name.starts_with(&local_image_name) {
        image_name
    } else {
        local_image_name.as_str()
    };
    format!("{base_image_name}-uid")
}

fn uid_update_local_image_name(workspace_folder: &Path) -> String {
    let basename = workspace_folder
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("workspace")
        .chars()
        .flat_map(|character| character.to_lowercase())
        .map(|character| {
            if character.is_ascii_lowercase() || character.is_ascii_digit() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let hash = Sha256::digest(workspace_folder.to_string_lossy().as_bytes());
    format!("vsc-{basename}-{hash:x}")
}

fn uid_update_base_image(args: &[String], image_name: &str) -> String {
    if uses_podman_engine(args) && !has_registry_hostname(image_name) {
        return format!("localhost/{image_name}");
    }
    image_name.to_string()
}

fn uses_podman_engine(args: &[String]) -> bool {
    Path::new(&engine::effective_engine_program(args))
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("podman"))
}

fn has_registry_hostname(image_name: &str) -> bool {
    if image_name.starts_with("localhost/") {
        return true;
    }
    let dot = image_name.find('.');
    let slash = image_name.find('/');
    dot.is_some_and(|dot| slash.is_some_and(|slash| dot < slash))
}

fn is_uid_update_platform_supported() -> bool {
    cfg!(target_os = "linux")
}

fn unique_uid_update_build_context() -> PathBuf {
    unique_temp_path("devcontainer-update-uid", None)
}

fn host_uid_gid() -> Result<(String, String), String> {
    let id_program = ["/usr/bin/id", "/bin/id"]
        .into_iter()
        .find(|program| Path::new(program).exists())
        .unwrap_or("id");
    let uid = command_stdout(id_program, &["-u"])?;
    let gid = command_stdout(id_program, &["-g"])?;
    Ok((uid, gid))
}

fn command_stdout(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests;
