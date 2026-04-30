//! Compose service inspection and build metadata helpers.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use serde_yaml::{Mapping, Value as YamlValue};

use super::ComposeSpec;
use crate::runtime::engine;
use crate::runtime::paths::resolve_relative;

pub(super) struct ServiceDefinition {
    pub(super) image: Option<String>,
    pub(super) has_build: bool,
    pub(super) build: Option<ServiceBuildInfo>,
    pub(super) user: Option<String>,
    pub(super) entrypoint: Option<Vec<String>>,
    pub(super) command: Option<Vec<String>>,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct ServiceBuildInfo {
    pub(super) context: String,
    pub(super) dockerfile_path: String,
    pub(super) target: Option<String>,
    pub(super) args: Option<HashMap<String, String>>,
}

pub(super) fn compose_files(
    configuration: &Value,
    config_root: &Path,
    workspace_root: &Path,
) -> Result<Vec<PathBuf>, String> {
    match configuration.get("dockerComposeFile") {
        Some(Value::String(value)) => Ok(vec![resolve_relative(config_root, value)]),
        Some(Value::Array(values)) if !values.is_empty() => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(|path| resolve_relative(config_root, path))
                    .ok_or_else(|| "dockerComposeFile entries must be strings".to_string())
            })
            .collect(),
        Some(Value::Array(_)) => default_compose_files(workspace_root),
        Some(_) => Err("dockerComposeFile must be a string or array of strings".to_string()),
        None => Err("Compose configuration must define dockerComposeFile".to_string()),
    }
}

fn default_compose_files(workspace_root: &Path) -> Result<Vec<PathBuf>, String> {
    if let Some(compose_files) =
        compose_files_from_env(std::env::var_os("COMPOSE_FILE"), workspace_root)
    {
        return Ok(compose_files);
    }

    let env_file = workspace_root.join(".env");
    if let Ok(raw) = fs::read_to_string(&env_file) {
        if let Some(value) = raw.lines().find_map(|line| {
            line.trim()
                .strip_prefix("COMPOSE_FILE=")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        }) {
            if let Some(compose_files) =
                compose_files_from_env(Some(OsString::from(value)), workspace_root)
            {
                return Ok(compose_files);
            }
        }
    }

    let mut files = vec![workspace_root.join("docker-compose.yml")];
    let override_file = workspace_root.join("docker-compose.override.yml");
    if override_file.is_file() {
        files.push(override_file);
    }
    Ok(files)
}

fn compose_files_from_env(value: Option<OsString>, workspace_root: &Path) -> Option<Vec<PathBuf>> {
    let value = value?;
    let files = std::env::split_paths(&value)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                workspace_root.join(path)
            }
        })
        .collect::<Vec<_>>();
    (!files.is_empty()).then_some(files)
}

pub(super) fn inspect_service_definition(
    compose_files: &[PathBuf],
    service: &str,
) -> Result<ServiceDefinition, String> {
    let mut image = None;
    let mut has_build = false;
    let mut build = None;
    let mut user = None;
    let mut entrypoint = None;
    let mut command = None;
    let mut found_service = false;
    let default_build_context = compose_files
        .first()
        .and_then(|path| path.parent())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| ".".to_string());

    for compose_file in compose_files {
        let raw = std::fs::read_to_string(compose_file).map_err(|error| error.to_string())?;
        let parsed: YamlValue = serde_yaml::from_str(&raw).map_err(|error| error.to_string())?;
        let Some(service_definition) = parsed
            .as_mapping()
            .and_then(|root| root.get(YamlValue::String("services".to_string())))
            .and_then(YamlValue::as_mapping)
            .and_then(|services| services.get(YamlValue::String(service.to_string())))
            .and_then(YamlValue::as_mapping)
        else {
            continue;
        };

        found_service = true;

        if service_definition.contains_key(YamlValue::String("build".to_string())) {
            has_build = true;
        }
        if let Some(value) = service_field(service_definition, "build") {
            build = parse_service_build(value, &default_build_context);
        }
        if let Some(value) = service_field(service_definition, "image").and_then(YamlValue::as_str)
        {
            image = Some(value.to_string());
        }
        if let Some(value) = service_field(service_definition, "user").and_then(YamlValue::as_str) {
            user = Some(value.to_string());
        }
        if let Some(value) =
            service_field(service_definition, "entrypoint").and_then(parse_service_command)
        {
            entrypoint = Some(value);
        }
        if let Some(value) =
            service_field(service_definition, "command").and_then(parse_service_command)
        {
            command = Some(value);
        }
    }

    if !found_service {
        return Err(format!(
            "Unable to locate compose service `{service}` in compose configuration"
        ));
    }

    Ok(ServiceDefinition {
        image,
        has_build,
        build,
        user,
        entrypoint,
        command,
    })
}

fn service_field<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a YamlValue> {
    mapping.get(YamlValue::String(key.to_string()))
}

fn parse_service_build(value: &YamlValue, default_context: &str) -> Option<ServiceBuildInfo> {
    match value {
        YamlValue::String(context) => Some(ServiceBuildInfo {
            context: context.to_string(),
            dockerfile_path: "Dockerfile".to_string(),
            target: None,
            args: None,
        }),
        YamlValue::Mapping(mapping) => Some(ServiceBuildInfo {
            context: service_field(mapping, "context")
                .and_then(YamlValue::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| default_context.to_string()),
            dockerfile_path: service_field(mapping, "dockerfile")
                .and_then(YamlValue::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| "Dockerfile".to_string()),
            target: service_field(mapping, "target")
                .and_then(YamlValue::as_str)
                .map(str::to_string),
            args: service_field(mapping, "args").and_then(parse_build_args),
        }),
        _ => None,
    }
}

fn parse_build_args(value: &YamlValue) -> Option<HashMap<String, String>> {
    let mapping = value.as_mapping()?;
    let args = mapping
        .iter()
        .filter_map(|(key, value)| {
            let key = yaml_scalar_to_string(key)?;
            let value = yaml_scalar_to_string(value)?;
            Some((key, value))
        })
        .collect::<HashMap<_, _>>();
    (!args.is_empty()).then_some(args)
}

pub(super) fn read_version_prefix(compose_files: &[PathBuf]) -> Result<String, String> {
    let Some(first_compose_file) = compose_files.first() else {
        return Ok(String::new());
    };
    let raw = fs::read_to_string(first_compose_file).map_err(|error| error.to_string())?;
    let version = raw.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix("version:")
            .map(|_| line.trim())
    });
    Ok(version
        .filter(|value| !value.is_empty())
        .map(|value| format!("{value}\n\n"))
        .unwrap_or_default())
}

fn parse_service_command(value: &YamlValue) -> Option<Vec<String>> {
    match value {
        YamlValue::String(text) => Some(split_shell_words(text)),
        YamlValue::Sequence(values) => Some(
            values
                .iter()
                .filter_map(yaml_scalar_to_string)
                .collect::<Vec<_>>(),
        ),
        YamlValue::Null => Some(Vec::new()),
        _ => None,
    }
}

fn yaml_scalar_to_string(value: &YamlValue) -> Option<String> {
    match value {
        YamlValue::String(text) => Some(text.to_string()),
        YamlValue::Bool(value) => Some(value.to_string()),
        YamlValue::Number(value) => Some(value.to_string()),
        YamlValue::Null => Some(String::new()),
        _ => None,
    }
}

fn split_shell_words(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut characters = value.chars().peekable();
    let mut quote = None;

    while let Some(character) = characters.next() {
        match quote {
            Some('\'') => {
                if character == '\'' {
                    quote = None;
                } else {
                    current.push(character);
                }
            }
            Some('"') => {
                if character == '"' {
                    quote = None;
                } else if character == '\\' {
                    if let Some(next) = characters.next() {
                        current.push(next);
                    }
                } else {
                    current.push(character);
                }
            }
            _ if character.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ if character == '\'' || character == '"' => {
                quote = Some(character);
            }
            _ if character == '\\' => {
                if let Some(next) = characters.next() {
                    current.push(next);
                }
            }
            _ => current.push(character),
        }
    }

    if let Some(quote) = quote {
        current.insert(0, quote);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

pub(super) fn default_service_image_name(spec: &ComposeSpec, args: &[String]) -> String {
    format!(
        "{}{}{}",
        spec.project_name,
        compose_image_name_separator(args),
        spec.service
    )
}

pub(super) fn compose_image_name_separator(args: &[String]) -> char {
    let Ok(result) = engine::run_compose(args, vec!["version".to_string(), "--short".to_string()])
    else {
        return '-';
    };
    if result.status_code != 0 {
        return '-';
    }

    let Some((major, minor, patch)) = parse_semver_prefix(result.stdout.trim()) else {
        return '-';
    };
    if (major, minor, patch) < (2, 8, 0) {
        '_'
    } else {
        '-'
    }
}

pub(super) fn parse_semver_prefix(value: &str) -> Option<(u64, u64, u64)> {
    let normalized = value.trim_start_matches('v');
    let version = normalized
        .split(|character: char| !(character.is_ascii_digit() || character == '.'))
        .next()
        .filter(|value| !value.is_empty())?;
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}
