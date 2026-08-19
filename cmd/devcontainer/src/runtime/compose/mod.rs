//! Compose-backed runtime orchestration for build, up, and container discovery.

mod args;
mod override_file;
mod project;
mod service;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;
use serde_yaml::{Mapping, Value as YamlValue};

use crate::commands::common;
use crate::commands::configuration;

use super::context::ResolvedConfig;
use super::engine;
use super::paths::unique_temp_path;

const COMPOSE_PROJECT_LABEL: &str = "com.docker.compose.project";
const COMPOSE_SERVICE_LABEL: &str = "com.docker.compose.service";

#[derive(Debug)]
pub(crate) struct ComposeSpec {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) service: String,
    pub(crate) image: Option<String>,
    pub(crate) has_build: bool,
    pub(crate) user: Option<String>,
    pub(crate) project_name: String,
}

#[derive(Debug)]
pub(crate) struct ComposeUpResult {
    pub(crate) project_name: String,
    pub(crate) service: String,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

pub(crate) fn uses_compose_config(configuration: &Value) -> bool {
    configuration.get("dockerComposeFile").is_some()
        && configuration
            .get("service")
            .and_then(Value::as_str)
            .is_some()
}

pub(crate) fn load_compose_spec(resolved: &ResolvedConfig) -> Result<Option<ComposeSpec>, String> {
    let Some(service) = resolved
        .configuration
        .get("service")
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    if resolved.configuration.get("dockerComposeFile").is_none() {
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
    let service = service.to_string();
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
    if let Some(feature_support) = &feature_support {
        configuration::validate_native_lockfile(
            args,
            &resolved.config_file,
            &resolved.configuration,
            feature_support,
        )?;
    }

    if engine::pull_always_requested(args) {
        pull_compose_images(resolved, args, &spec)?;
    }

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
        let installations = &feature_support.installations;
        super::build::build_feature_image(
            args,
            &built_image,
            &compose_image,
            installations,
            false,
        )?;
        let configuration = &resolved.configuration;
        configuration::ensure_native_lockfile(
            args,
            &resolved.config_file,
            configuration,
            &feature_support,
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
) -> Result<ComposeUpResult, String> {
    let spec = load_compose_spec(resolved)?
        .ok_or_else(|| "Compose configuration was expected but not found".to_string())?;
    let selected_services = compose_services_to_start(resolved, &spec);
    let profile_override_file = compose_profile_override_file(&spec, selected_services.as_deref())?;
    let override_file = match override_file::compose_metadata_override_file(
        resolved,
        args,
        remote_workspace_folder,
        if spec.image.as_deref() != Some(image_name) || spec.has_build {
            Some(image_name)
        } else {
            None
        },
    ) {
        Ok(override_file) => override_file,
        Err(error) => {
            remove_compose_override(profile_override_file.as_ref());
            return Err(error);
        }
    };
    let mut up_args = vec!["-d".to_string()];
    if no_recreate {
        up_args.push("--no-recreate".to_string());
    }
    if let Some(services) = selected_services {
        up_args.extend(services);
    }
    let tail_len = up_args.len();
    let mut compose_args = args::compose_args_owned(&spec, "up", override_file.as_ref(), up_args);
    if let Some(profile_override_file) = &profile_override_file {
        insert_compose_override(&mut compose_args, tail_len, profile_override_file);
    }
    let result = engine::run_compose(args, compose_args);
    remove_compose_override(override_file.as_ref());
    remove_compose_override(profile_override_file.as_ref());
    let result = result?;
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }
    Ok(ComposeUpResult {
        project_name: spec.project_name,
        service: spec.service,
        stdout: result.stdout,
        stderr: result.stderr,
    })
}

fn pull_compose_images(
    resolved: &ResolvedConfig,
    args: &[String],
    spec: &ComposeSpec,
) -> Result<(), String> {
    let selected_services = compose_services_to_start(resolved, spec);
    let profile_override_file = compose_profile_override_file(spec, selected_services.as_deref())?;
    let result = pull_compose_images_with_override(
        args,
        spec,
        selected_services.as_deref(),
        profile_override_file.as_ref(),
    );
    remove_compose_override(profile_override_file.as_ref());
    result
}

fn pull_compose_images_with_override(
    args: &[String],
    spec: &ComposeSpec,
    selected_services: Option<&[String]>,
    profile_override_file: Option<&PathBuf>,
) -> Result<(), String> {
    let config_args = args::compose_args_owned(spec, "config", profile_override_file, Vec::new());
    let result = engine::run_compose(args, config_args)?;
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }
    let compose_config: YamlValue = serde_yaml::from_str(&result.stdout)
        .map_err(|error| format!("Unable to parse resolved Compose configuration: {error}"))?;
    let services = compose_config
        .as_mapping()
        .and_then(|root| yaml_field(root, "services"))
        .and_then(YamlValue::as_mapping)
        .ok_or_else(|| "Resolved Compose configuration does not define services".to_string())?;
    let services_to_pull = remote_service_closure(services, selected_services)?;
    let mut pulled_images = HashSet::new();
    for service in services_to_pull {
        let definition = services
            .get(YamlValue::String(service))
            .and_then(YamlValue::as_mapping)
            .expect("remote service closure only returns defined services");
        let image = yaml_field(definition, "image")
            .and_then(YamlValue::as_str)
            .expect("remote service closure only returns services with images");
        let platform = yaml_field(definition, "platform")
            .and_then(YamlValue::as_str)
            .filter(|platform| !platform.trim().is_empty());
        if !pulled_images.insert((image.to_string(), platform.map(str::to_string))) {
            continue;
        }
        let mut pull_args = vec!["pull".to_string()];
        if let Some(platform) = platform {
            pull_args.extend(["--platform".to_string(), platform.to_string()]);
        }
        pull_args.push(image.to_string());
        let result = engine::run_engine(args, pull_args)?;
        if result.status_code != 0 {
            return Err(engine::stderr_or_stdout(&result));
        }
    }
    Ok(())
}

fn compose_profile_override_file(
    spec: &ComposeSpec,
    selected_services: Option<&[String]>,
) -> Result<Option<PathBuf>, String> {
    if selected_services.is_none() {
        return Ok(None);
    }
    let profiled_services = compose_profiled_services(spec)?;
    if profiled_services.is_empty() {
        return Ok(None);
    }

    let mut content = service::read_version_prefix(&spec.files)?;
    content.push_str("services:\n");
    for service in profiled_services {
        content.push_str(&format!(
            "  '{}':\n    profiles: !reset []\n",
            service.replace('\'', "''")
        ));
    }
    let path = unique_temp_path("devcontainer-compose-profile-override", Some("yml"));
    std::fs::write(&path, content).map_err(|error| error.to_string())?;
    Ok(Some(path))
}

fn compose_profiled_services(spec: &ComposeSpec) -> Result<Vec<String>, String> {
    let mut profiled_services = HashSet::new();
    for compose_file in &spec.files {
        let raw = std::fs::read_to_string(compose_file).map_err(|error| error.to_string())?;
        let parsed: YamlValue = serde_yaml::from_str(&raw).map_err(|error| error.to_string())?;
        let Some(services) = parsed
            .as_mapping()
            .and_then(|root| yaml_field(root, "services"))
            .and_then(YamlValue::as_mapping)
        else {
            continue;
        };
        for (service, definition) in services {
            let (Some(service), Some(definition)) = (service.as_str(), definition.as_mapping())
            else {
                continue;
            };
            let Some(profiles) = yaml_field(definition, "profiles") else {
                continue;
            };
            match compose_profiles_state(profiles) {
                Some(true) => {
                    profiled_services.insert(service.to_string());
                }
                Some(false) => {
                    profiled_services.remove(service);
                }
                None => {}
            }
        }
    }
    let mut profiled_services = profiled_services.into_iter().collect::<Vec<_>>();
    profiled_services.sort();
    Ok(profiled_services)
}

fn compose_profiles_state(profiles: &YamlValue) -> Option<bool> {
    match profiles {
        YamlValue::Sequence(profiles) if !profiles.is_empty() => Some(true),
        YamlValue::Sequence(_) => None,
        YamlValue::Tagged(tagged) if tagged.tag == "!reset" => {
            Some(compose_profiles_state(&tagged.value).unwrap_or(false))
        }
        YamlValue::Tagged(tagged) => compose_profiles_state(&tagged.value),
        YamlValue::Null => Some(false),
        _ => None,
    }
}

fn insert_compose_override(args: &mut Vec<String>, tail_len: usize, override_file: &Path) {
    let subcommand_index = args.len() - tail_len - 1;
    args.splice(
        subcommand_index..subcommand_index,
        ["-f".to_string(), override_file.display().to_string()],
    );
}

fn remove_compose_override(override_file: Option<&PathBuf>) {
    if let Some(override_file) = override_file {
        let _ = std::fs::remove_file(override_file);
    }
}

fn compose_services_to_start(resolved: &ResolvedConfig, spec: &ComposeSpec) -> Option<Vec<String>> {
    let run_services = resolved
        .configuration
        .get("runServices")
        .and_then(Value::as_array)
        .filter(|services| !services.is_empty())?;
    let mut services = run_services
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    if !services.iter().any(|service| service == &spec.service) {
        services.push(spec.service.clone());
    }
    Some(services)
}

fn remote_service_closure(
    services: &Mapping,
    selected_services: Option<&[String]>,
) -> Result<Vec<String>, String> {
    let roots = match selected_services {
        Some(services) => services.to_vec(),
        None => services
            .keys()
            .filter_map(YamlValue::as_str)
            .map(str::to_string)
            .collect(),
    };
    let mut visited = HashSet::new();
    let mut remote_services = Vec::new();
    for service in roots {
        visit_remote_service(services, &service, &mut visited, &mut remote_services)?;
    }
    Ok(remote_services)
}

fn visit_remote_service(
    services: &Mapping,
    service: &str,
    visited: &mut HashSet<String>,
    remote_services: &mut Vec<String>,
) -> Result<(), String> {
    if !visited.insert(service.to_string()) {
        return Ok(());
    }
    let definition = services
        .get(YamlValue::String(service.to_string()))
        .and_then(YamlValue::as_mapping)
        .ok_or_else(|| {
            format!("Unable to locate compose service `{service}` in resolved configuration")
        })?;
    for dependency in compose_service_dependencies(definition) {
        visit_remote_service(services, &dependency, visited, remote_services)?;
    }

    let has_remote_image = yaml_field(definition, "image")
        .and_then(YamlValue::as_str)
        .is_some_and(|image| !image.trim().is_empty());
    let has_build = yaml_field(definition, "build").is_some_and(|build| !build.is_null());
    if has_remote_image && !has_build {
        remote_services.push(service.to_string());
    }
    Ok(())
}

fn compose_service_dependencies(definition: &Mapping) -> Vec<String> {
    let mut dependencies = Vec::new();
    if let Some(depends_on) = yaml_field(definition, "depends_on") {
        match depends_on {
            YamlValue::Mapping(mapping) => dependencies.extend(
                mapping
                    .keys()
                    .filter_map(YamlValue::as_str)
                    .map(str::to_string),
            ),
            YamlValue::Sequence(sequence) => dependencies.extend(
                sequence
                    .iter()
                    .filter_map(YamlValue::as_str)
                    .map(str::to_string),
            ),
            _ => {}
        }
    }
    for field in ["links", "volumes_from"] {
        if let Some(values) = yaml_field(definition, field).and_then(YamlValue::as_sequence) {
            dependencies.extend(values.iter().filter_map(YamlValue::as_str).filter_map(
                |reference| {
                    let service = reference.split(':').next()?;
                    (service != "container" && !service.is_empty()).then(|| service.to_string())
                },
            ));
        }
    }
    for field in ["ipc", "network_mode", "pid"] {
        if let Some(service) = yaml_field(definition, field)
            .and_then(YamlValue::as_str)
            .and_then(|reference| reference.strip_prefix("service:"))
        {
            dependencies.push(service.to_string());
        }
    }
    dependencies
}

fn yaml_field<'a>(mapping: &'a Mapping, field: &str) -> Option<&'a YamlValue> {
    mapping.get(YamlValue::String(field.to_string()))
}

pub(crate) fn service_logs(
    resolved: &ResolvedConfig,
    args: &[String],
) -> Result<crate::process_runner::ProcessResult, String> {
    let spec = load_compose_spec(resolved)?
        .ok_or_else(|| "Compose configuration was expected but not found".to_string())?;
    engine::run_compose(
        args,
        args::compose_args_owned(&spec, "logs", None, vec![spec.service.clone()]),
    )
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
    let mut ps_args = vec!["ps".to_string(), "-q".to_string()];
    if include_stopped {
        ps_args.push("-a".to_string());
    }
    ps_args.push("--filter".to_string());
    ps_args.push(format!(
        "label={COMPOSE_PROJECT_LABEL}={}",
        spec.project_name
    ));
    ps_args.push("--filter".to_string());
    ps_args.push(format!("label={COMPOSE_SERVICE_LABEL}={}", spec.service));

    let result = engine::run_engine(args, ps_args)?;
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
