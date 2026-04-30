//! Compose-backed runtime orchestration for build, up, and container discovery.

mod args;
mod override_file;
mod project;
mod service;

use std::path::PathBuf;

use serde_json::Value;

use crate::commands::common;
use crate::commands::configuration;

use super::context::ResolvedConfig;
use super::engine;

pub(crate) struct ComposeSpec {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) service: String,
    pub(crate) image: Option<String>,
    pub(crate) has_build: bool,
    pub(crate) user: Option<String>,
    pub(crate) project_name: String,
}

pub(crate) fn uses_compose_config(configuration: &Value) -> bool {
    configuration.get("dockerComposeFile").is_some()
        && configuration
            .get("service")
            .and_then(Value::as_str)
            .is_some()
}

pub(crate) fn load_compose_spec(resolved: &ResolvedConfig) -> Result<Option<ComposeSpec>, String> {
    if !uses_compose_config(&resolved.configuration) {
        return Ok(None);
    }

    let config_root = resolved
        .config_file
        .parent()
        .unwrap_or(resolved.workspace_folder.as_path());
    let files = service::compose_files(
        &resolved.configuration,
        config_root,
        &resolved.workspace_folder,
    )?;
    let service = resolved
        .configuration
        .get("service")
        .and_then(Value::as_str)
        .ok_or_else(|| "Compose configuration must define service".to_string())?
        .to_string();
    let project_name = project::compose_project_name(&files)?;
    let definition = service::inspect_service_definition(&files, &service)?;
    let has_build = definition.has_build || definition.build.is_some();

    Ok(Some(ComposeSpec {
        files,
        service,
        image: definition.image,
        has_build,
        user: definition.user,
        project_name,
    }))
}

pub(crate) fn build_service(resolved: &ResolvedConfig, args: &[String]) -> Result<String, String> {
    let spec = load_compose_spec(resolved)?
        .ok_or_else(|| "Compose configuration was expected but not found".to_string())?;
    args::reject_unsupported_build_options(args)?;
    let feature_support = configuration::resolve_feature_support(
        args,
        &resolved.workspace_folder,
        &resolved.config_file,
        &resolved.configuration,
    )?;

    if spec.has_build {
        let build_override_file = override_file::compose_build_override_file(&spec, args)?;
        let mut build_args = vec!["--pull".to_string()];
        if common::has_flag(args, "--no-cache") || common::has_flag(args, "--build-no-cache") {
            build_args.push("--no-cache".to_string());
        }
        build_args.push(spec.service.clone());
        let result = engine::run_compose(
            args,
            args::compose_args_owned(&spec, "build", build_override_file.as_ref(), build_args),
        );
        if let Some(build_override_file) = build_override_file {
            let _ = std::fs::remove_file(build_override_file);
        }
        let result = result?;
        if result.status_code != 0 {
            return Err(engine::stderr_or_stdout(&result));
        }
    }

    let compose_image = spec
        .image
        .clone()
        .unwrap_or_else(|| service::default_service_image_name(&spec, args));
    if let Some(feature_support) = feature_support {
        let built_image = common::parse_option_value(args, "--image-name")
            .unwrap_or_else(|| compose_image.clone());
        super::build::build_feature_image(
            args,
            &built_image,
            &compose_image,
            &feature_support.installations,
        )?;
        if common::has_flag(args, "--push") {
            let push_result =
                engine::run_engine(args, vec!["push".to_string(), built_image.clone()])?;
            if push_result.status_code != 0 {
                return Err(engine::stderr_or_stdout(&push_result));
            }
        }
        configuration::ensure_native_lockfile(
            args,
            &resolved.config_file,
            &resolved.configuration,
        )?;
        return Ok(built_image);
    }

    Ok(spec
        .image
        .clone()
        .unwrap_or_else(|| service::default_service_image_name(&spec, args)))
}

pub(crate) fn up_service(
    resolved: &ResolvedConfig,
    args: &[String],
    remote_workspace_folder: &str,
    image_name: &str,
    no_recreate: bool,
) -> Result<(), String> {
    let spec = load_compose_spec(resolved)?
        .ok_or_else(|| "Compose configuration was expected but not found".to_string())?;
    let override_file = override_file::compose_metadata_override_file(
        resolved,
        args,
        remote_workspace_folder,
        if spec.image.as_deref() != Some(image_name) || spec.has_build {
            Some(image_name)
        } else {
            None
        },
    )?;
    let mut up_args = vec!["-d".to_string()];
    if no_recreate {
        up_args.push("--no-recreate".to_string());
    }
    if let Some(run_services) = resolved
        .configuration
        .get("runServices")
        .and_then(Value::as_array)
        .filter(|services| !services.is_empty())
    {
        let mut has_primary_service = false;
        for service in run_services.iter().filter_map(Value::as_str) {
            has_primary_service |= service == spec.service;
            up_args.push(service.to_string());
        }
        if !has_primary_service {
            up_args.push(spec.service.clone());
        }
    }
    let result = engine::run_compose(
        args,
        args::compose_args_owned(&spec, "up", override_file.as_ref(), up_args),
    )?;
    if let Some(override_file) = override_file {
        let _ = std::fs::remove_file(override_file);
    }
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }
    Ok(())
}

pub(crate) fn remove_service(resolved: &ResolvedConfig, args: &[String]) -> Result<(), String> {
    let spec = load_compose_spec(resolved)?
        .ok_or_else(|| "Compose configuration was expected but not found".to_string())?;
    let result = engine::run_compose(
        args,
        args::compose_args(&spec, "rm", &["-s", "-f", &spec.service]),
    )?;
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }
    Ok(())
}

pub(crate) fn resolve_container_id(
    resolved: &ResolvedConfig,
    args: &[String],
) -> Result<Option<String>, String> {
    resolve_container_id_with_options(resolved, args, false)
}

pub(crate) fn resolve_container_id_including_stopped(
    resolved: &ResolvedConfig,
    args: &[String],
) -> Result<Option<String>, String> {
    resolve_container_id_with_options(resolved, args, true)
}

fn resolve_container_id_with_options(
    resolved: &ResolvedConfig,
    args: &[String],
    include_stopped: bool,
) -> Result<Option<String>, String> {
    let spec = load_compose_spec(resolved)?
        .ok_or_else(|| "Compose configuration was expected but not found".to_string())?;
    let mut ps_args = vec!["-q"];
    if include_stopped {
        ps_args.push("-a");
    }
    ps_args.push(&spec.service);
    let result = engine::run_compose(args, args::compose_args(&spec, "ps", &ps_args))?;
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }

    Ok(result
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.chars().any(char::is_whitespace))
        .map(str::to_string))
}

#[cfg(test)]
mod tests;
