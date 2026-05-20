//! Native image build orchestration for image, Dockerfile, and feature flows.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::commands::common;
use crate::commands::configuration;

use super::compose;
use super::context::ResolvedConfig;
use super::engine;
use super::paths::{resolve_relative, unique_temp_path};

pub(crate) fn runtime_image_name(
    resolved: &ResolvedConfig,
    args: &[String],
) -> Result<String, String> {
    let has_native_features = configuration::resolve_feature_support(
        args,
        &resolved.workspace_folder,
        &resolved.config_file,
        &resolved.configuration,
    )?
    .is_some();
    if compose::uses_compose_config(&resolved.configuration) {
        compose::build_service(resolved, args)
    } else if has_build_definition(&resolved.configuration) || has_native_features {
        build_image(resolved, args)
    } else if let Some(image) = resolved.configuration.get("image").and_then(Value::as_str) {
        Ok(image.to_string())
    } else {
        Err(
            "Unsupported configuration: only image and build-based configs are supported natively"
                .to_string(),
        )
    }
}

pub(crate) fn build_image(resolved: &ResolvedConfig, args: &[String]) -> Result<String, String> {
    if compose::uses_compose_config(&resolved.configuration) {
        return compose::build_service(resolved, args);
    }

    let feature_support = configuration::resolve_feature_support(
        args,
        &resolved.workspace_folder,
        &resolved.config_file,
        &resolved.configuration,
    )?;
    if !has_build_definition(&resolved.configuration) {
        let image = resolved
            .configuration
            .get("image")
            .and_then(Value::as_str)
            .map(|value| value.to_string())
            .ok_or_else(|| {
                "Unsupported configuration: only image and build-based configs are supported natively"
                    .to_string()
            })?;
        return if let Some(feature_support) = feature_support {
            configuration::validate_native_lockfile(
                args,
                &resolved.config_file,
                &resolved.configuration,
                &feature_support,
            )?;
            let image_name = common::parse_option_value(args, "--image-name")
                .unwrap_or_else(|| default_image_name(&resolved.workspace_folder));
            let built =
                build_feature_image(args, &image_name, &image, &feature_support.installations)?;
            maybe_push_image(args, &built)?;
            configuration::ensure_native_lockfile(
                args,
                &resolved.config_file,
                &resolved.configuration,
                &feature_support,
            )?;
            Ok(built)
        } else {
            Ok(image)
        };
    }

    let image_name = common::parse_option_value(args, "--image-name")
        .unwrap_or_else(|| default_image_name(&resolved.workspace_folder));
    if let Some(feature_support) = feature_support {
        configuration::validate_native_lockfile(
            args,
            &resolved.config_file,
            &resolved.configuration,
            &feature_support,
        )?;
        let base_image = format!("{image_name}-base");
        build_base_image(resolved, args, &base_image)?;
        let built = build_feature_image(
            args,
            &image_name,
            &base_image,
            &feature_support.installations,
        )?;
        maybe_push_image(args, &built)?;
        configuration::ensure_native_lockfile(
            args,
            &resolved.config_file,
            &resolved.configuration,
            &feature_support,
        )?;
        return Ok(built);
    }

    build_base_image(resolved, args, &image_name)?;
    maybe_push_image(args, &image_name)?;
    Ok(image_name)
}

fn build_base_image(
    resolved: &ResolvedConfig,
    args: &[String],
    image_name: &str,
) -> Result<(), String> {
    let build = resolved
        .configuration
        .get("build")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let config_root = resolved
        .config_file
        .parent()
        .unwrap_or(resolved.workspace_folder.as_path());
    let dockerfile = build
        .get("dockerfile")
        .or_else(|| build.get("dockerFile"))
        .and_then(Value::as_str)
        .unwrap_or("Dockerfile");
    let context = build.get("context").and_then(Value::as_str).unwrap_or(".");
    let dockerfile_path = resolve_relative(config_root, dockerfile);
    let context_path = resolve_relative(config_root, context);
    let mut engine_args = engine_build_args(args, image_name, &dockerfile_path);
    if let Some(build_args) = build.get("args").and_then(Value::as_object) {
        for (key, value) in build_args {
            if let Some(value) = value.as_str() {
                engine_args.push("--build-arg".to_string());
                engine_args.push(format!("{key}={value}"));
            }
        }
    }
    engine_args.push(context_path.display().to_string());

    let result = engine::run_engine(args, engine_args)?;
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }

    Ok(())
}

pub(crate) fn build_feature_image(
    args: &[String],
    image_name: &str,
    base_image: &str,
    installations: &[configuration::FeatureInstallation],
) -> Result<String, String> {
    let build_context_dir = unique_feature_build_dir();
    fs::create_dir_all(&build_context_dir).map_err(|error| error.to_string())?;
    let dockerfile_path =
        write_feature_dockerfile(args, &build_context_dir, base_image, installations)?;
    let mut engine_args = engine_build_args(args, image_name, &dockerfile_path);
    engine_args.push(build_context_dir.display().to_string());

    let result = engine::run_engine(args, engine_args);
    let cleanup = fs::remove_dir_all(&build_context_dir);
    let result = result?;
    if result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&result));
    }
    cleanup.map_err(|error| error.to_string())?;
    Ok(image_name.to_string())
}

fn maybe_push_image(args: &[String], image_name: &str) -> Result<(), String> {
    if !common::has_flag(args, "--push") {
        return Ok(());
    }

    let push_result = engine::run_engine(args, vec!["push".to_string(), image_name.to_string()])?;
    if push_result.status_code != 0 {
        return Err(engine::stderr_or_stdout(&push_result));
    }

    Ok(())
}

fn write_feature_dockerfile(
    args: &[String],
    build_context_dir: &Path,
    base_image: &str,
    installations: &[configuration::FeatureInstallation],
) -> Result<PathBuf, String> {
    let dockerfile_path = build_context_dir.join("Dockerfile");
    let mut dockerfile = format!("{}FROM {base_image}\n", dockerfile_prefix(args));
    for (index, installation) in installations.iter().enumerate() {
        let feature_name = configuration::feature_installation_name(installation);
        let destination = format!("feature-{index}-{feature_name}");
        let copied_feature_dir = build_context_dir.join(&destination);
        configuration::materialize_feature_installation(installation, &copied_feature_dir)?;
        let install_path = format!("/tmp/devcontainer-features/{destination}");
        dockerfile.push_str(&format!("COPY {destination} {install_path}\n"));
        let env_assignments = installation
            .env
            .iter()
            .map(|(key, value)| format!("{key}={}", shell_single_quote(value)))
            .collect::<Vec<_>>()
            .join(" ");
        let command = if env_assignments.is_empty() {
            "chmod +x install.sh && ./install.sh".to_string()
        } else {
            format!("chmod +x install.sh && {env_assignments} ./install.sh")
        };
        dockerfile.push_str(&format!(
            "RUN cd {install_path} && /bin/sh -lc {}\n",
            shell_single_quote(&command)
        ));
    }
    fs::write(&dockerfile_path, dockerfile).map_err(|error| error.to_string())?;
    Ok(dockerfile_path)
}

fn dockerfile_prefix(args: &[String]) -> &'static str {
    if common::runtime_options(args).omit_syntax_directive {
        ""
    } else {
        "# syntax=docker/dockerfile:1.4\n"
    }
}

fn engine_build_args(args: &[String], image_name: &str, dockerfile_path: &Path) -> Vec<String> {
    let mut engine_args = vec![
        "build".to_string(),
        "--tag".to_string(),
        image_name.to_string(),
        "--file".to_string(),
        dockerfile_path.display().to_string(),
    ];
    let cache_to_values = common::parse_option_values(args, "--cache-to");
    if common::has_flag(args, "--no-cache") || common::has_flag(args, "--build-no-cache") {
        engine_args.push("--no-cache".to_string());
    }
    for value in common::parse_option_values(args, "--cache-from") {
        engine_args.push("--cache-from".to_string());
        engine_args.push(value);
    }
    for value in &cache_to_values {
        engine_args.push("--cache-to".to_string());
        engine_args.push(value.clone());
    }
    if !cache_to_values
        .iter()
        .any(|value| is_buildx_cache_to_inline(Some(value)))
    {
        engine_args.push("--build-arg".to_string());
        engine_args.push("BUILDKIT_INLINE_CACHE=1".to_string());
    }
    for value in common::parse_option_values(args, "--label") {
        engine_args.push("--label".to_string());
        engine_args.push(value);
    }
    if let Some(platform) = common::parse_option_value(args, "--platform") {
        engine_args.push("--platform".to_string());
        engine_args.push(platform);
    }
    engine_args
}

fn is_buildx_cache_to_inline(buildx_cache_to: Option<&str>) -> bool {
    let Some(buildx_cache_to) = buildx_cache_to else {
        return false;
    };
    let mut value = buildx_cache_to;
    while let Some(index) = value.to_ascii_lowercase().find("type") {
        value = &value[index + "type".len()..];
        let trimmed = value.trim_start();
        let Some(after_equals) = trimmed.strip_prefix('=') else {
            value = trimmed.get(1..).unwrap_or_default();
            continue;
        };
        let target = after_equals.trim_start();
        if target
            .get(.."inline".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("inline"))
        {
            return true;
        }
        value = target;
    }
    false
}

fn unique_feature_build_dir() -> PathBuf {
    unique_temp_path("devcontainer-feature-build", None)
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn default_image_name(workspace_folder: &Path) -> String {
    let basename = workspace_folder
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("workspace")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("devcontainer-{basename}")
}

fn has_build_definition(configuration: &Value) -> bool {
    configuration
        .get("build")
        .is_some_and(|value| value.is_object())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::json;

    use crate::runtime::context::ResolvedConfig;
    use crate::test_support::{unique_temp_dir, write_executable_script};

    use super::{
        build_image, default_image_name, dockerfile_prefix, engine_build_args,
        has_build_definition, is_buildx_cache_to_inline, runtime_image_name, shell_single_quote,
    };

    fn contains_arg(args: &[String], expected: &str) -> bool {
        args.iter().any(|arg| arg == expected)
    }

    #[test]
    fn is_buildx_cache_to_inline_matches_upstream_cases() {
        assert!(!is_buildx_cache_to_inline(None));
        assert!(!is_buildx_cache_to_inline(Some("")));

        assert!(is_buildx_cache_to_inline(Some("type=inline")));
        assert!(is_buildx_cache_to_inline(Some("type = inline")));
        assert!(is_buildx_cache_to_inline(Some("type=INLINE")));
        assert!(is_buildx_cache_to_inline(Some(
            "mode=max,type=inline,compression=zstd"
        )));

        assert!(!is_buildx_cache_to_inline(Some("type=registry")));
        assert!(!is_buildx_cache_to_inline(Some("type=local")));
        assert!(!is_buildx_cache_to_inline(Some("inline")));
    }

    #[test]
    fn engine_build_args_adds_inline_cache_build_arg_by_default() {
        let engine_args = engine_build_args(&[], "example/native:test", Path::new("Dockerfile"));

        assert!(contains_arg(&engine_args, "--build-arg"));
        assert!(contains_arg(&engine_args, "BUILDKIT_INLINE_CACHE=1"));
    }

    #[test]
    fn engine_build_args_suppresses_inline_cache_build_arg_for_inline_cache_to() {
        let engine_args = engine_build_args(
            &[
                "--cache-to".to_string(),
                "mode=max,type=inline,compression=zstd".to_string(),
            ],
            "example/native:test",
            Path::new("Dockerfile"),
        );

        assert!(contains_arg(&engine_args, "--cache-to"));
        assert!(contains_arg(
            &engine_args,
            "mode=max,type=inline,compression=zstd"
        ));
        assert!(!contains_arg(&engine_args, "BUILDKIT_INLINE_CACHE=1"));
    }

    #[test]
    fn engine_build_args_keeps_inline_cache_build_arg_for_non_inline_cache_to() {
        let engine_args = engine_build_args(
            &[
                "--cache-to".to_string(),
                "type=registry,ref=ghcr.io/example/cache:latest".to_string(),
            ],
            "example/native:test",
            Path::new("Dockerfile"),
        );

        assert!(contains_arg(&engine_args, "--cache-to"));
        assert!(contains_arg(
            &engine_args,
            "type=registry,ref=ghcr.io/example/cache:latest"
        ));
        assert!(contains_arg(&engine_args, "BUILDKIT_INLINE_CACHE=1"));
    }

    #[test]
    fn engine_build_args_treats_any_inline_cache_to_as_inline() {
        let engine_args = engine_build_args(
            &[
                "--cache-to".to_string(),
                "type=registry,ref=ghcr.io/example/cache:latest".to_string(),
                "--cache-to".to_string(),
                "type=inline".to_string(),
            ],
            "example/native:test",
            Path::new("Dockerfile"),
        );

        assert!(!contains_arg(&engine_args, "BUILDKIT_INLINE_CACHE=1"));
    }

    #[test]
    fn engine_build_args_include_cache_label_platform_and_no_cache_flags() {
        let engine_args = engine_build_args(
            &[
                "--build-no-cache".to_string(),
                "--cache-from".to_string(),
                "type=registry,ref=ghcr.io/example/cache:old".to_string(),
                "--cache-to".to_string(),
                "type=registry,ref=ghcr.io/example/cache:new".to_string(),
                "--label".to_string(),
                "devcontainer.test=true".to_string(),
                "--platform".to_string(),
                "linux/arm64".to_string(),
            ],
            "example/native:test",
            Path::new("Dockerfile"),
        );

        assert!(contains_arg(&engine_args, "--no-cache"));
        assert!(contains_arg(&engine_args, "--cache-from"));
        assert!(contains_arg(
            &engine_args,
            "type=registry,ref=ghcr.io/example/cache:old"
        ));
        assert!(contains_arg(&engine_args, "--cache-to"));
        assert!(contains_arg(
            &engine_args,
            "type=registry,ref=ghcr.io/example/cache:new"
        ));
        assert!(contains_arg(&engine_args, "--label"));
        assert!(contains_arg(&engine_args, "devcontainer.test=true"));
        assert!(contains_arg(&engine_args, "--platform"));
        assert!(contains_arg(&engine_args, "linux/arm64"));
    }

    #[test]
    fn build_helpers_cover_prefix_names_and_config_detection() {
        assert_eq!(dockerfile_prefix(&[]), "# syntax=docker/dockerfile:1.4\n");
        assert_eq!(
            dockerfile_prefix(&["--omit-syntax-directive".to_string()]),
            ""
        );
        assert_eq!(shell_single_quote("it's ok"), "'it'\"'\"'s ok'");
        assert_eq!(
            default_image_name(Path::new("/tmp/My Workspace!")),
            "devcontainer-My-Workspace-"
        );
        assert!(has_build_definition(&json!({
            "build": {}
        })));
        assert!(!has_build_definition(&json!({
            "build": "Dockerfile"
        })));
    }

    #[test]
    fn build_image_runs_engine_build_with_config_args_and_optional_push() {
        let root = unique_temp_dir("devcontainer-build-runtime-test");
        let config_dir = root.join(".devcontainer");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::write(config_dir.join("Dockerfile"), "FROM alpine:3.20\n").expect("dockerfile");
        let fake_engine = root.join("docker");
        let log = root.join("engine.log");
        write_executable_script(
            &fake_engine,
            &format!(
                r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> '{}'
exit 0
"#,
                log.display()
            ),
        );
        let resolved = ResolvedConfig {
            workspace_folder: root.clone(),
            config_file: config_dir.join("devcontainer.json"),
            configuration: json!({
                "build": {
                    "dockerfile": "Dockerfile",
                    "context": ".",
                    "args": {
                        "FOO": "bar",
                        "IGNORED": true
                    }
                }
            }),
        };
        let args = vec![
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
            "--image-name".to_string(),
            "example/native:test".to_string(),
            "--push".to_string(),
        ];

        let image_name = build_image(&resolved, &args).expect("build image");

        assert_eq!(image_name, "example/native:test");
        let invocations = fs::read_to_string(&log).expect("engine log");
        assert!(invocations.contains("build --tag example/native:test"));
        assert!(invocations.contains("--build-arg FOO=bar"));
        assert!(!invocations.contains("IGNORED=true"));
        assert!(invocations.contains("push example/native:test"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn runtime_image_name_returns_plain_images_and_reports_unsupported_configs() {
        let root = unique_temp_dir("devcontainer-runtime-image-test");
        fs::create_dir_all(&root).expect("workspace");
        let image_config = ResolvedConfig {
            workspace_folder: root.clone(),
            config_file: root.join(".devcontainer.json"),
            configuration: json!({
                "image": "alpine:3.20"
            }),
        };
        assert_eq!(
            runtime_image_name(&image_config, &[]).expect("image name"),
            "alpine:3.20"
        );

        let unsupported = ResolvedConfig {
            configuration: json!({}),
            ..image_config
        };
        assert!(runtime_image_name(&unsupported, &[])
            .expect_err("unsupported")
            .contains("Unsupported configuration"));
        let _ = fs::remove_dir_all(root);
    }
}
