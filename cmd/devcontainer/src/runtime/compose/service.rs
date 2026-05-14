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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use serde_json::json;

    use super::{
        compose_files, default_service_image_name, inspect_service_definition, parse_build_args,
        parse_semver_prefix, parse_service_build, parse_service_command, read_version_prefix,
        split_shell_words, yaml_scalar_to_string,
    };
    use crate::runtime::compose::ComposeSpec;

    #[test]
    fn compose_files_accept_strings_arrays_defaults_and_reject_invalid_entries() {
        let root = crate::test_support::unique_temp_dir("devcontainer-compose-service-test");
        let config_root = root.join(".devcontainer");
        fs::create_dir_all(&config_root).expect("config root");
        fs::write(
            root.join(".env"),
            "COMPOSE_FILE=compose.yml:sub/extra.yml\n",
        )
        .expect("env");

        assert_eq!(
            compose_files(
                &json!({"dockerComposeFile": "docker-compose.yml"}),
                &config_root,
                &root,
            )
            .expect("single file"),
            vec![config_root.join("docker-compose.yml")]
        );
        assert_eq!(
            compose_files(
                &json!({"dockerComposeFile": ["one.yml", "two.yml"]}),
                &config_root,
                &root,
            )
            .expect("array files"),
            vec![config_root.join("one.yml"), config_root.join("two.yml")]
        );
        assert_eq!(
            compose_files(&json!({"dockerComposeFile": []}), &config_root, &root)
                .expect("default files"),
            vec![root.join("compose.yml"), root.join("sub").join("extra.yml")]
        );
        assert!(
            compose_files(&json!({"dockerComposeFile": [1]}), &config_root, &root)
                .expect_err("invalid entry")
                .contains("entries must be strings")
        );
        assert!(
            compose_files(&json!({"dockerComposeFile": true}), &config_root, &root)
                .expect_err("invalid type")
                .contains("string or array")
        );
        assert!(compose_files(&json!({}), &config_root, &root)
            .expect_err("missing file")
            .contains("must define dockerComposeFile"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn compose_files_default_to_standard_files_and_optional_override() {
        let root = crate::test_support::unique_temp_dir("devcontainer-compose-service-test");
        fs::create_dir_all(&root).expect("workspace");
        fs::write(root.join("docker-compose.override.yml"), "services: {}\n").expect("override");

        let files =
            compose_files(&json!({"dockerComposeFile": []}), &root, &root).expect("default files");

        assert_eq!(
            files,
            vec![
                root.join("docker-compose.yml"),
                root.join("docker-compose.override.yml")
            ]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn inspect_service_definition_merges_files_and_parses_runtime_fields() {
        let root = crate::test_support::unique_temp_dir("devcontainer-compose-service-test");
        let base = root.join("base.yml");
        let override_file = root.join("override.yml");
        fs::create_dir_all(&root).expect("compose root");
        fs::write(
            &base,
            r#"
services:
  app:
    image: base-image
    build:
      context: ./context
      dockerfile: Dockerfile.dev
      target: runtime
      args:
        STRING_ARG: value
        BOOL_ARG: true
        NUMBER_ARG: 42
        NULL_ARG:
        ignored:
          nested: true
    user: "1000:1000"
    entrypoint: "/bin/sh -lc \"echo base\""
    command: ["sleep", 1, true, null, { ignored: true }]
"#,
        )
        .expect("base compose");
        fs::write(
            &override_file,
            r#"
services:
  app:
    image: override-image
    command: echo override
"#,
        )
        .expect("override compose");

        let definition =
            inspect_service_definition(&[base, override_file], "app").expect("definition");
        let build = definition.build.expect("build info");

        assert_eq!(definition.image.as_deref(), Some("override-image"));
        assert!(definition.has_build);
        assert_eq!(definition.user.as_deref(), Some("1000:1000"));
        assert_eq!(
            definition.entrypoint,
            Some(vec![
                "/bin/sh".to_string(),
                "-lc".to_string(),
                "echo base".to_string()
            ])
        );
        assert_eq!(
            definition.command,
            Some(vec!["echo".to_string(), "override".to_string()])
        );
        assert_eq!(build.context, "./context");
        assert_eq!(build.dockerfile_path, "Dockerfile.dev");
        assert_eq!(build.target.as_deref(), Some("runtime"));
        let args = build.args.expect("build args");
        assert_eq!(args.get("STRING_ARG").map(String::as_str), Some("value"));
        assert_eq!(args.get("BOOL_ARG").map(String::as_str), Some("true"));
        assert_eq!(args.get("NUMBER_ARG").map(String::as_str), Some("42"));
        assert_eq!(args.get("NULL_ARG").map(String::as_str), Some(""));
        assert!(!args.contains_key("ignored"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn inspect_service_definition_reports_missing_and_invalid_compose_files() {
        let root = crate::test_support::unique_temp_dir("devcontainer-compose-service-test");
        let compose_file = root.join("docker-compose.yml");
        fs::create_dir_all(&root).expect("compose root");
        fs::write(&compose_file, "services:\n  other:\n    image: alpine\n").expect("compose");

        let error = match inspect_service_definition(std::slice::from_ref(&compose_file), "app") {
            Ok(_) => panic!("missing service should fail"),
            Err(error) => error,
        };
        assert!(
            error.contains("Unable to locate compose service"),
            "{error}"
        );

        fs::write(&compose_file, "services: [").expect("invalid compose");
        let error = match inspect_service_definition(&[compose_file], "app") {
            Ok(_) => panic!("invalid yaml should fail"),
            Err(error) => error,
        };
        assert!(!error.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn service_build_and_command_parsers_handle_edge_shapes() {
        assert_eq!(
            parse_service_build(&serde_yaml::Value::Bool(true), "."),
            None
        );
        let build = parse_service_build(
            &serde_yaml::from_str(
                r#"
args:
  one: 1
  enabled: true
  blank:
  nested:
    skip: true
"#,
            )
            .expect("yaml"),
            "/workspace",
        )
        .expect("mapping build");
        assert_eq!(build.context, "/workspace");
        assert_eq!(build.dockerfile_path, "Dockerfile");
        assert_eq!(build.target, None);
        assert_eq!(build.args.expect("args").len(), 3);

        assert_eq!(
            parse_build_args(&serde_yaml::from_str("[one, two]").expect("yaml")),
            None
        );
        assert_eq!(
            parse_service_command(&serde_yaml::Value::Null),
            Some(Vec::new())
        );
        assert_eq!(parse_service_command(&serde_yaml::Value::Bool(true)), None);
        assert_eq!(
            yaml_scalar_to_string(&serde_yaml::Value::Bool(false)),
            Some("false".into())
        );
        assert_eq!(
            split_shell_words(
                r#"cmd "two words" 'literal value' escaped\ space "quoted\"value" 'unterminated"#
            ),
            vec![
                "cmd",
                "two words",
                "literal value",
                "escaped space",
                "quoted\"value",
                "'unterminated",
            ]
        );
    }

    #[test]
    fn version_prefix_and_image_name_helpers_cover_edge_cases() {
        let root = crate::test_support::unique_temp_dir("devcontainer-compose-service-test");
        let compose_file = root.join("docker-compose.yml");
        fs::create_dir_all(&root).expect("compose root");
        fs::write(
            &compose_file,
            "name: app\nversion: '3.9'\nservices:\n  app:\n    image: alpine\n",
        )
        .expect("compose");

        assert_eq!(
            read_version_prefix(std::slice::from_ref(&compose_file)).expect("version"),
            "version: '3.9'\n\n"
        );
        assert_eq!(read_version_prefix(&[]).expect("empty"), "");
        assert_eq!(parse_semver_prefix("v2.8"), Some((2, 8, 0)));
        assert_eq!(parse_semver_prefix("2"), Some((2, 0, 0)));
        assert_eq!(parse_semver_prefix("not-a-version"), None);

        let spec = ComposeSpec {
            files: vec![PathBuf::from("docker-compose.yml")],
            service: "web".to_string(),
            image: None,
            has_build: false,
            user: None,
            project_name: "myproj".to_string(),
        };
        assert_eq!(default_service_image_name(&spec, &[]), "myproj-web");

        let _ = fs::remove_dir_all(root);
    }
}
