//! Unit tests for compose runtime helpers.

use serde_json::json;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use super::override_file::{compose_build_override_file, compose_metadata_override_file};
use super::project::{
    compose_name_from_file, compose_project_name, sanitize_project_name,
    substitute_compose_env_with,
};
use super::service::{
    compose_image_name_separator, inspect_service_definition, parse_semver_prefix,
};
use super::uses_compose_config;
use super::{
    build_service, load_compose_spec, resolve_container_id, resolve_container_id_including_stopped,
    up_service, ComposeSpec,
};
use crate::runtime::context::ResolvedConfig;
use crate::test_support::{
    init_git_repo, process_env_guard, unique_temp_dir, write_executable_script,
};

static PATH_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct PathEnvGuard {
    original: Option<std::ffi::OsString>,
}

impl Drop for PathEnvGuard {
    fn drop(&mut self) {
        match &self.original {
            Some(value) => env::set_var("PATH", value),
            None => env::remove_var("PATH"),
        }
    }
}

fn with_host_tool_path<T>(action: impl FnOnce() -> T) -> T {
    let _guard = PATH_ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("PATH env lock");
    let original = env::var_os("PATH");
    env::set_var("PATH", host_tool_path(original.as_ref()));
    let _path_guard = PathEnvGuard { original };
    action()
}

fn host_tool_path(original: Option<&std::ffi::OsString>) -> std::ffi::OsString {
    let mut paths = Vec::new();
    if let Some(parent) = git_executable().parent() {
        paths.push(parent.to_path_buf());
    }
    paths.extend(
        ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
            .iter()
            .map(PathBuf::from),
    );
    if let Some(original) = original {
        paths.extend(env::split_paths(original));
    }
    env::join_paths(paths).expect("host tool PATH")
}

fn git_executable() -> PathBuf {
    [
        "/usr/bin/git",
        "/opt/homebrew/bin/git",
        "/usr/local/bin/git",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
    .unwrap_or_else(|| PathBuf::from("git"))
}

fn resolved_config(
    workspace_folder: std::path::PathBuf,
    configuration: serde_json::Value,
) -> ResolvedConfig {
    ResolvedConfig {
        config_file: workspace_folder.join(".devcontainer.json"),
        workspace_folder,
        configuration,
    }
}

#[test]
fn detects_compose_configs() {
    assert!(uses_compose_config(&json!({
        "dockerComposeFile": "docker-compose.yml",
        "service": "app"
    })));
    assert!(!uses_compose_config(&json!({
        "image": "alpine:3.20"
    })));
}

#[test]
fn load_compose_spec_skips_non_compose_configs_and_reports_invalid_compose_files() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");

    let non_compose = resolved_config(
        root.clone(),
        json!({
            "image": "alpine:3.20"
        }),
    );
    assert!(load_compose_spec(&non_compose)
        .expect("non-compose config")
        .is_none());

    let service_without_compose_file = resolved_config(
        root.clone(),
        json!({
            "service": "app"
        }),
    );
    assert!(load_compose_spec(&service_without_compose_file)
        .expect("service without compose file")
        .is_none());

    let invalid = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": true,
            "service": "app"
        }),
    );
    assert!(load_compose_spec(&invalid)
        .expect_err("invalid dockerComposeFile")
        .contains("string or array"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_compose_spec_reports_missing_compose_files_and_services() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");

    let missing_file = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "missing.yml",
            "service": "app"
        }),
    );
    let error = load_compose_spec(&missing_file).expect_err("missing compose file");
    assert!(!error.is_empty());

    let compose_file = root.join("docker-compose.yml");
    fs::write(
        &compose_file,
        "services:\n  other:\n    image: alpine:3.20\n",
    )
    .expect("compose");
    let missing_service = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    );
    let error = load_compose_spec(&missing_service).expect_err("missing compose service");
    assert!(
        error.contains("Unable to locate compose service"),
        "{error}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn compose_entrypoints_report_non_compose_configs() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = resolved_config(
        root.clone(),
        json!({
            "image": "alpine:3.20"
        }),
    );

    assert!(build_service(&resolved, &[])
        .expect_err("build service")
        .contains("expected but not found"));
    assert!(
        up_service(&resolved, &[], "/workspace", "alpine:3.20", false)
            .expect_err("up service")
            .contains("expected but not found")
    );
    assert!(resolve_container_id(&resolved, &[])
        .expect_err("resolve container")
        .contains("expected but not found"));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn inspects_service_image_and_build_presence() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let compose_file = root.join("docker-compose.yml");
    fs::create_dir_all(&root).expect("compose root");
    fs::write(
        &compose_file,
        "services:\n  app:\n    image: example/native-compose:test\n    build:\n      context: .\n",
    )
    .expect("compose file");

    let definition =
        inspect_service_definition(&[compose_file], "app").expect("service definition");

    assert_eq!(
        definition.image.as_deref(),
        Some("example/native-compose:test")
    );
    assert!(definition.has_build);
    assert_eq!(definition.user, None);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn inspects_service_build_info_for_upstream_compose_shapes() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let compose_dir = root.join("somepath");
    let compose_file = compose_dir.join("docker-compose.yml");
    fs::create_dir_all(&compose_dir).expect("compose root");
    fs::write(
        &compose_file,
        r#"
services:
  fully_specified:
    image: my-image
    build:
      context: context-path
      dockerfile: my-dockerfile
      target: a-target
      args:
        arg1: value1
  image_only:
    image: my-image
  string_build:
    image: my-image
    build: ./a-path
  default_dockerfile:
    build:
      context: ./a-path
  default_context:
    build:
      dockerfile: my-dockerfile
"#,
    )
    .expect("compose file");

    let fully_specified =
        inspect_service_definition(std::slice::from_ref(&compose_file), "fully_specified")
            .expect("fully specified service");
    let fully_specified_build = fully_specified.build.as_ref().expect("build info");
    assert_eq!(fully_specified.image.as_deref(), Some("my-image"));
    assert_eq!(fully_specified_build.context, "context-path");
    assert_eq!(fully_specified_build.dockerfile_path, "my-dockerfile");
    assert_eq!(fully_specified_build.target.as_deref(), Some("a-target"));
    assert_eq!(
        fully_specified_build
            .args
            .as_ref()
            .and_then(|args| args.get("arg1"))
            .map(String::as_str),
        Some("value1")
    );

    let image_only = inspect_service_definition(std::slice::from_ref(&compose_file), "image_only")
        .expect("image-only service");
    assert_eq!(image_only.image.as_deref(), Some("my-image"));
    assert!(image_only.build.is_none());

    let string_build =
        inspect_service_definition(std::slice::from_ref(&compose_file), "string_build")
            .expect("string build service");
    let string_build_info = string_build.build.as_ref().expect("string build info");
    assert_eq!(string_build.image.as_deref(), Some("my-image"));
    assert_eq!(string_build_info.context, "./a-path");
    assert_eq!(string_build_info.dockerfile_path, "Dockerfile");
    assert_eq!(string_build_info.target, None);
    assert_eq!(string_build_info.args, None);

    let default_dockerfile =
        inspect_service_definition(std::slice::from_ref(&compose_file), "default_dockerfile")
            .expect("default dockerfile service");
    let default_dockerfile_build = default_dockerfile.build.as_ref().expect("build info");
    assert_eq!(default_dockerfile_build.context, "./a-path");
    assert_eq!(default_dockerfile_build.dockerfile_path, "Dockerfile");
    assert_eq!(default_dockerfile_build.target, None);
    assert_eq!(default_dockerfile_build.args, None);

    let default_context =
        inspect_service_definition(std::slice::from_ref(&compose_file), "default_context")
            .expect("default context service");
    let default_context_build = default_context.build.as_ref().expect("build info");
    assert_eq!(
        default_context_build.context,
        compose_dir.display().to_string()
    );
    assert_eq!(default_context_build.dockerfile_path, "my-dockerfile");
    assert_eq!(default_context_build.target, None);
    assert_eq!(default_context_build.args, None);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn compose_project_name_defaults_to_workspace_devcontainer() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let compose_file = root.join(".devcontainer").join("docker-compose.yml");
    fs::create_dir_all(compose_file.parent().expect("compose dir")).expect("compose dir");
    fs::write(&compose_file, "services:\n  app:\n    image: alpine:3.20\n").expect("compose");

    let project_name = compose_project_name(&[compose_file]).expect("project name");

    assert_eq!(
        project_name,
        root.file_name().unwrap().to_string_lossy().to_lowercase() + "_devcontainer"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn compose_project_name_defaults_to_compose_working_dir_basename() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let compose_file = root.join("docker-compose.yml");
    fs::create_dir_all(&root).expect("compose dir");
    fs::write(&compose_file, "services:\n  app:\n    image: alpine:3.20\n").expect("compose");

    let project_name = compose_project_name(&[compose_file]).expect("project name");

    assert_eq!(
        project_name,
        root.file_name().unwrap().to_string_lossy().to_lowercase()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn compose_project_name_reports_missing_files_and_sanitizes_names() {
    assert_eq!(sanitize_project_name("My Project! 123"), "myproject123");
    assert!(compose_project_name(&[])
        .expect_err("missing compose files should fail")
        .contains("at least one compose file"));
}

#[test]
fn compose_project_name_reads_dotenv_and_reports_read_errors() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let compose_dir = root.join("compose");
    let compose_file = compose_dir.join("docker-compose.yml");
    fs::create_dir_all(&compose_dir).expect("compose dir");
    fs::write(&compose_file, "services:\n  app:\n    image: alpine:3.20\n").expect("compose");
    fs::write(
        compose_dir.join(".env"),
        "\n# comment\nCOMPOSE_PROJECT_NAME=Env_Project\n",
    )
    .expect("env file");

    let project_name =
        compose_project_name(std::slice::from_ref(&compose_file)).expect("dotenv project name");
    assert_eq!(project_name, "env_project");

    let unreadable_dir = compose_dir.join(".env");
    let _ = fs::remove_file(&unreadable_dir);
    fs::create_dir(&unreadable_dir).expect("env directory");
    assert!(!compose_project_name(&[compose_file])
        .expect_err("directory env file should fail")
        .is_empty());

    let _ = fs::remove_dir_all(root);
}

#[test]
fn compose_project_name_reads_top_level_name_from_compose_files() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let compose_file = root.join("docker-compose.yml");
    fs::create_dir_all(&root).expect("compose dir");
    fs::write(
        &compose_file,
        "name: Named-Project\nservices:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");

    let project_name =
        compose_project_name(std::slice::from_ref(&compose_file)).expect("project name");

    assert_eq!(project_name, "named-project");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn compose_project_name_honors_environment_before_files() {
    let mut env_guard = process_env_guard();
    env_guard.set_var("COMPOSE_PROJECT_NAME", "Env Project!");

    let project_name = compose_project_name(&[]).expect("env project name");

    assert_eq!(project_name, "envproject");
}

#[test]
fn compose_project_name_ignores_blank_environment_values() {
    let mut env_guard = process_env_guard();
    env_guard.set_var("COMPOSE_PROJECT_NAME", "  ");
    let root = unique_temp_dir("devcontainer-compose-test");
    let compose_file = root.join("docker-compose.yml");
    fs::create_dir_all(&root).expect("compose dir");
    fs::write(&compose_file, "services:\n  app:\n    image: alpine:3.20\n").expect("compose");

    let project_name =
        compose_project_name(std::slice::from_ref(&compose_file)).expect("project name");

    assert_eq!(
        project_name,
        root.file_name().unwrap().to_string_lossy().to_lowercase()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn compose_name_from_file_reads_top_level_name() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let compose_file = root.join("docker-compose.yml");
    fs::create_dir_all(&root).expect("compose dir");
    fs::write(
        &compose_file,
        "name: Custom-Project-Name\nservices:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose");

    let project_name = compose_name_from_file(&compose_file)
        .expect("compose name")
        .expect("top-level name");

    assert_eq!(project_name, "Custom-Project-Name");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn compose_name_from_file_reports_read_errors() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let compose_file = root.join("missing.yml");

    let error = compose_name_from_file(&compose_file).expect_err("missing compose file");

    assert!(!error.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn compose_name_from_file_ignores_nested_names_and_unquotes_values() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let compose_file = root.join("docker-compose.yml");
    fs::create_dir_all(&root).expect("compose dir");
    fs::write(
        &compose_file,
        "services:\n  app:\n    name: Nested\nname: 'Quoted-Project'\n",
    )
    .expect("compose");

    let project_name = compose_name_from_file(&compose_file)
        .expect("compose name")
        .expect("top-level name");

    assert_eq!(project_name, "Quoted-Project");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn compose_name_from_file_supports_colon_dash_default_interpolation() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let compose_file = root.join("docker-compose.yml");
    let variable = format!("DEVCONTAINER_COMPOSE_TEST_MISSING_{}_A", std::process::id());
    fs::create_dir_all(&root).expect("compose dir");
    fs::write(
        &compose_file,
        format!("name: ${{{variable}:-MyProj}}\nservices:\n  app:\n    image: alpine:3.20\n"),
    )
    .expect("compose");

    let project_name = compose_name_from_file(&compose_file)
        .expect("compose name")
        .expect("top-level name");

    assert_eq!(project_name, "MyProj");
    assert_eq!(sanitize_project_name(&project_name), "myproj");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn compose_name_from_file_supports_dash_default_interpolation() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let compose_file = root.join("docker-compose.yml");
    let variable = format!("DEVCONTAINER_COMPOSE_TEST_MISSING_{}_B", std::process::id());
    fs::create_dir_all(&root).expect("compose dir");
    fs::write(
        &compose_file,
        format!("name: ${{{variable}-MyProj}}\nservices:\n  app:\n    image: alpine:3.20\n"),
    )
    .expect("compose");

    let project_name = compose_name_from_file(&compose_file)
        .expect("compose name")
        .expect("top-level name");

    assert_eq!(project_name, "MyProj");
    assert_eq!(sanitize_project_name(&project_name), "myproj");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn substitute_compose_env_supports_plain_variable_interpolation() {
    let variable = "DEVCONTAINER_COMPOSE_TEST_PRESENT";
    let lookup = |name: &str| (name == variable).then_some("MyProject".to_string());

    assert_eq!(
        substitute_compose_env_with(&format!("prefix-${variable}"), &lookup),
        "prefix-MyProject"
    );
    assert_eq!(substitute_compose_env_with("cost-$$5", &lookup), "cost-$5");
    assert_eq!(
        substitute_compose_env_with("literal-$", &lookup),
        "literal-$"
    );
    assert_eq!(
        substitute_compose_env_with("literal-$9", &lookup),
        "literal-$9"
    );
    assert_eq!(
        substitute_compose_env_with("${UNFINISHED", &lookup),
        "${UNFINISHED"
    );
    assert_eq!(
        substitute_compose_env_with("${DEVCONTAINER_COMPOSE_TEST_PRESENT:-fallback}", &lookup),
        "MyProject"
    );
    assert_eq!(
        substitute_compose_env_with("${DEVCONTAINER_COMPOSE_TEST_PRESENT-fallback}", &lookup),
        "MyProject"
    );
    assert_eq!(
        substitute_compose_env_with("'${MISSING:-fallback}'", &lookup),
        "fallback"
    );
}

#[test]
fn parse_semver_prefix_reads_plain_semver_versions() {
    assert_eq!(parse_semver_prefix("2.24.0"), Some((2, 24, 0)));
    assert_eq!(parse_semver_prefix("v2.8.1-desktop.1"), Some((2, 8, 1)));
}

#[test]
fn compose_image_name_separator_defaults_to_hyphen_without_runtime_args() {
    assert_eq!(compose_image_name_separator(&[]), '-');
}

#[test]
fn compose_image_name_separator_tracks_compose_version_outputs() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let compose_path = root.join("compose.sh");

    write_executable_script(
        &compose_path,
        "#!/bin/sh\nif [ \"${1:-}\" = version ]; then\n  printf '2.7.9\\n'\n  exit 0\nfi\nexit 1\n",
    );
    assert_eq!(
        compose_image_name_separator(&[
            "--docker-compose-path".to_string(),
            compose_path.display().to_string()
        ]),
        '_'
    );

    write_executable_script(
        &compose_path,
        "#!/bin/sh\nif [ \"${1:-}\" = version ]; then\n  printf '2.8.0\\n'\n  exit 0\nfi\nexit 1\n",
    );
    assert_eq!(
        compose_image_name_separator(&[
            "--docker-compose-path".to_string(),
            compose_path.display().to_string()
        ]),
        '-'
    );

    write_executable_script(
        &compose_path,
        "#!/bin/sh\nif [ \"${1:-}\" = version ]; then\n  printf 'not-a-version\\n'\n  exit 0\nfi\nexit 1\n",
    );
    assert_eq!(
        compose_image_name_separator(&[
            "--docker-compose-path".to_string(),
            compose_path.display().to_string()
        ]),
        '-'
    );

    write_executable_script(
        &compose_path,
        "#!/bin/sh\nif [ \"${1:-}\" = version ]; then\n  echo failed >&2\n  exit 7\nfi\nexit 1\n",
    );
    assert_eq!(
        compose_image_name_separator(&[
            "--docker-compose-path".to_string(),
            compose_path.display().to_string()
        ]),
        '-'
    );

    assert_eq!(
        compose_image_name_separator(&[
            "--docker-compose-path".to_string(),
            root.join("missing-compose").display().to_string()
        ]),
        '-'
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn compose_build_override_file_renders_cache_from_values_with_version() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let compose_file = root.join("docker-compose.yml");
    fs::create_dir_all(&root).expect("compose dir");
    fs::write(
        &compose_file,
        "version: '3.9'\nservices:\n  app:\n    build: .\n",
    )
    .expect("compose");
    let spec = ComposeSpec {
        files: vec![compose_file],
        service: "app".to_string(),
        image: None,
        has_build: true,
        user: None,
        project_name: "project".to_string(),
    };

    assert!(compose_build_override_file(&spec, &[])
        .expect("empty cache-from")
        .is_none());
    let override_file = compose_build_override_file(
        &spec,
        &[
            "--cache-from".to_string(),
            "type=registry,ref=example/cache:$latest".to_string(),
        ],
    )
    .expect("cache-from override")
    .expect("override file");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(override_content.starts_with("version: '3.9'\n\n"));
    assert!(override_content.contains("cache_from:"));
    assert!(override_content.contains("type=registry,ref=example/cache:$$latest"));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_does_not_mount_workspace_by_default() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "workspaceFolder": "/workspace/dataplattformen"
        }),
    };

    let override_file =
        compose_metadata_override_file(&resolved, &[], "/workspace/dataplattformen", None)
            .expect("override result")
            .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(!override_content.contains("\n    volumes:\n"));
    assert!(!override_content.contains(&format!(
        "- '{}:/workspace/dataplattformen'",
        root.display()
    )));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_reports_missing_service_and_gpu_detection_errors() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let missing_service = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml"
        }),
    );
    assert!(
        compose_metadata_override_file(&missing_service, &[], "/workspace", None)
            .expect_err("missing service")
            .contains("must define service")
    );

    let gpu_config = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "hostRequirements": {
                "gpu": "optional"
            }
        }),
    );
    let missing_engine = root.join("missing-engine");
    let error = compose_metadata_override_file(
        &gpu_config,
        &[
            "--docker-path".to_string(),
            missing_engine.display().to_string(),
        ],
        "/workspace",
        None,
    )
    .expect_err("gpu detection should fail");
    assert!(error.contains("missing-engine"), "{error}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_skips_non_bind_workspace_mounts() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "workspaceMount": "type=volume,source=workspace-cache,target=/workspace"
        }),
    );

    let override_file = compose_metadata_override_file(&resolved, &[], "/workspace", None)
        .expect("override result")
        .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(!override_content.contains("\n    volumes:\n"));
    assert!(!override_content.contains("\nvolumes:\n"));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_still_renders_when_compose_context_cannot_be_loaded() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": true,
            "service": "app",
            "workspaceMount": "type=volume,source=workspace-cache,target=/workspace"
        }),
    );

    let override_file = compose_metadata_override_file(&resolved, &[], "/workspace", None)
        .expect("override result")
        .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(override_content.starts_with("services:\n"));
    assert!(override_content.contains("  'app':"));
    assert!(override_content.contains("entrypoint:"));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_preserves_compose_command_without_duplicate_override() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n    command: sleep infinity\n",
    )
    .expect("compose");
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "workspaceMount": "type=volume,source=workspace-cache,target=/workspace"
        }),
    );

    let override_file = compose_metadata_override_file(&resolved, &[], "/workspace", None)
        .expect("override result")
        .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(override_content.contains("entrypoint:"));
    assert!(!override_content.contains("command:"));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_clears_command_when_compose_entrypoint_would_consume_it() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n    entrypoint: /entrypoint.sh\n",
    )
    .expect("compose");
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "workspaceMount": "type=volume,source=workspace-cache,target=/workspace"
        }),
    );

    let override_file = compose_metadata_override_file(&resolved, &[], "/workspace", None)
        .expect("override result")
        .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(override_content.contains(r#"command: []"#));
    assert!(override_content.contains(r#""/entrypoint.sh""#));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_does_not_mount_nested_workspaces_from_the_git_root() {
    with_host_tool_path(|| {
        let root = unique_temp_dir("devcontainer-compose-test");
        let repo_root = root.join("repo");
        let workspace = repo_root.join("packages").join("app");
        fs::create_dir_all(&workspace).expect("workspace root");
        init_git_repo(&repo_root);
        let expected_repo_root = repo_root
            .canonicalize()
            .unwrap_or_else(|_| repo_root.clone());
        let resolved = crate::runtime::context::ResolvedConfig {
            workspace_folder: workspace,
            config_file: expected_repo_root
                .join("packages")
                .join("app")
                .join(".devcontainer.json"),
            configuration: json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app",
            }),
        };

        let override_file =
            compose_metadata_override_file(&resolved, &[], "/workspaces/repo/packages/app", None)
                .expect("override result")
                .expect("override path");
        let override_content = fs::read_to_string(&override_file).expect("override content");

        assert!(!override_content.contains("\n    volumes:\n"));
        assert!(!override_content.contains(&format!(
            "- '{}:/workspaces/repo'",
            expected_repo_root.display()
        )));

        let _ = fs::remove_file(override_file);
        let _ = fs::remove_dir_all(root);
    });
}

#[test]
fn metadata_override_file_does_not_add_workspace_or_git_common_dir_mounts() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "workspaceFolder": "/workspace",
        }),
    };

    let override_file = compose_metadata_override_file(
        &resolved,
        &["--mount-git-worktree-common-dir".to_string()],
        "/workspace",
        None,
    )
    .expect("override result")
    .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(!override_content.contains("\n    volumes:\n"));
    assert!(!override_content.contains(&format!("- '{}:/workspace'", root.display())));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_can_pin_image_and_runtime_settings() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "containerEnv": {
                "FEATURE_FLAG": "enabled"
            },
            "containerUser": "node",
            "remoteUser": "vscode",
            "privileged": true,
            "init": true,
            "capAdd": ["SYS_ADMIN"],
            "securityOpt": ["seccomp=unconfined"],
            "mounts": [{
                "type": "volume",
                "source": "feature-cache",
                "target": "/cache"
            }, "type=bind,source=/tmp/feature-src,target=/tmp/feature-dst,readonly"]
        }),
    };

    let override_file = compose_metadata_override_file(
        &resolved,
        &[],
        "/workspaces/project",
        Some("example/compose-featured:test"),
    )
    .expect("override result")
    .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(override_content.contains("image: 'example/compose-featured:test'"));
    assert!(override_content.contains("environment:"));
    assert!(override_content.contains("FEATURE_FLAG: 'enabled'"));
    assert!(override_content.contains("user: 'node'"));
    assert!(override_content.contains("privileged: true"));
    assert!(override_content.contains("init: true"));
    assert!(override_content.contains("cap_add:"));
    assert!(override_content.contains("security_opt:"));
    assert!(override_content.contains("type: 'volume'"));
    assert!(override_content.contains("source: 'feature-cache'"));
    assert!(override_content.contains("target: '/cache'"));
    assert!(override_content.contains("type: 'bind'"));
    assert!(override_content.contains("source: '/tmp/feature-src'"));
    assert!(override_content.contains("target: '/tmp/feature-dst'"));
    assert!(override_content.contains("read_only: true"));
    assert!(!override_content.contains("type=volume,source=feature-cache,target=/cache"));
    assert!(!override_content
        .contains("type=bind,source=/tmp/feature-src,target=/tmp/feature-dst,readonly"));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_wraps_entrypoints_with_a_keepalive_entrypoint() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "overrideCommand": true,
            "entrypoints": ["echo feature-entry", "echo feature-post-start"]
        }),
    };

    let override_file = compose_metadata_override_file(&resolved, &[], "/workspace", None)
        .expect("override result")
        .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(override_content.contains("entrypoint:"));
    assert!(override_content.contains("Container started"));
    assert!(override_content.contains("echo feature-entry"));
    assert!(override_content.contains("echo feature-post-start"));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_merges_config_entrypoint_into_wrapper_without_duplicates() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "entrypoint": "echo config-entrypoint"
        }),
    };

    let override_file = compose_metadata_override_file(&resolved, &[], "/workspace", None)
        .expect("override result")
        .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");
    let entrypoint_count = override_content
        .lines()
        .filter(|line| line.trim_start().starts_with("entrypoint:"))
        .count();

    assert_eq!(entrypoint_count, 1, "{override_content}");
    assert!(override_content.contains("Container started"));
    assert!(override_content.contains("echo config-entrypoint"));
    assert!(!override_content.contains("entrypoint: 'echo config-entrypoint'"));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_declares_named_volumes_top_level() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "mounts": [{
                "type": "volume",
                "source": "feature-cache",
                "target": "/cache",
                "external": true
            }]
        }),
    };

    let override_file = compose_metadata_override_file(&resolved, &[], "/workspace", None)
        .expect("override result")
        .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(override_content.contains("\nvolumes:\n"));
    assert!(override_content.contains("feature-cache:"));
    assert!(override_content.contains("external: true"));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_ignores_compose_workspace_mount() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "workspaceMount": "type=bind,source=/tmp/workspace,target=/workspaces/project,consistency=delegated"
        }),
    };

    let override_file = compose_metadata_override_file(&resolved, &[], "/", None)
        .expect("override result")
        .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(!override_content.contains("\n    volumes:\n"));
    assert!(!override_content.contains("/tmp/workspace"));
    assert!(!override_content.contains("/workspaces/project"));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_preserves_extended_mount_keys() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "mounts": [{
                "type": "bind",
                "source": "/tmp/feature-src",
                "target": "/tmp/feature-dst",
                "consistency": "delegated",
                "bind": {
                    "propagation": "rshared"
                }
            }, {
                "type": "volume",
                "source": "feature-cache",
                "target": "/cache",
                "external": true,
                "volume": {
                    "nocopy": true
                }
            }]
        }),
    };

    let override_file = compose_metadata_override_file(&resolved, &[], "/workspaces/project", None)
        .expect("override result")
        .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(override_content.contains("consistency: 'delegated'"));
    assert!(override_content.contains("bind:"));
    assert!(override_content.contains("propagation: 'rshared'"));
    assert!(override_content.contains("volume:"));
    assert!(override_content.contains("nocopy: true"));
    assert!(override_content.contains("external: true"));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_allows_anonymous_cli_volume_mounts() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    };

    let override_file = compose_metadata_override_file(
        &resolved,
        &[
            "--mount".to_string(),
            "type=volume,target=/cache".to_string(),
        ],
        "/workspaces/project",
        None,
    )
    .expect("override result")
    .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(override_content.contains("type: 'volume'"));
    assert!(override_content.contains("target: '/cache'"));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_appends_cli_mounts_after_config_mounts() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "mounts": [{
                "type": "bind",
                "source": "/tmp/config-src",
                "target": "/tmp/config-dst"
            }]
        }),
    };

    let override_file = compose_metadata_override_file(
        &resolved,
        &[
            "--mount".to_string(),
            "type=bind,source=/tmp/cli-src,target=/tmp/cli-dst,readonly".to_string(),
            "--mount".to_string(),
            "type=volume,source=cli-cache,target=/cli-cache".to_string(),
        ],
        "/workspaces/project",
        None,
    )
    .expect("override result")
    .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(override_content.contains("source: '/tmp/config-src'"));
    assert!(override_content.contains("target: '/tmp/config-dst'"));
    assert!(override_content.contains("source: '/tmp/cli-src'"));
    assert!(override_content.contains("target: '/tmp/cli-dst'"));
    assert!(override_content.contains("read_only: true"));
    assert!(override_content.contains("source: 'cli-cache'"));
    assert!(override_content.contains("target: '/cli-cache'"));

    let config_position = override_content
        .find("source: '/tmp/config-src'")
        .expect("config mount");
    let cli_position = override_content
        .find("source: '/tmp/cli-src'")
        .expect("cli mount");
    assert!(
        config_position < cli_position,
        "expected config mounts before CLI mounts: {override_content}"
    );

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_does_not_promote_remote_user_to_service_user() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "remoteUser": "vscode",
        }),
    };

    let override_file = compose_metadata_override_file(&resolved, &[], "/workspaces/project", None)
        .expect("override result")
        .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(!override_content.contains("user: 'vscode'"));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_adds_gpu_resources_when_requested() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "hostRequirements": {
                "gpu": "required"
            }
        }),
    };

    let override_file = compose_metadata_override_file(
        &resolved,
        &["--gpu-availability".to_string(), "all".to_string()],
        "/workspaces/project",
        None,
    )
    .expect("override result")
    .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(override_content.contains("deploy:"));
    assert!(override_content.contains("capabilities: [gpu]"));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_service_rejects_label_and_invalid_additional_features_before_engine_work() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n    build:\n      context: .\n",
    )
    .expect("compose file");
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    );

    let label_error = build_service(
        &resolved,
        &["--label".to_string(), "example=value".to_string()],
    )
    .expect_err("label should be rejected");
    assert_eq!(label_error, "--label not supported for compose builds.");

    let cache_to_error = build_service(
        &resolved,
        &["--cache-to".to_string(), "type=inline".to_string()],
    )
    .expect_err("cache-to should be rejected");
    assert_eq!(
        cache_to_error,
        "--cache-to not supported for compose builds."
    );

    let platform_error = build_service(
        &resolved,
        &["--platform".to_string(), "linux/arm64".to_string()],
    )
    .expect_err("platform should be rejected");
    assert_eq!(platform_error, "--platform or --push not supported.");

    let output_error = build_service(
        &resolved,
        &["--output".to_string(), "type=docker".to_string()],
    )
    .expect_err("output should be rejected");
    assert_eq!(output_error, "--output not supported.");

    let additional_features_error = build_service(
        &resolved,
        &["--additional-features".to_string(), "[]".to_string()],
    )
    .expect_err("additional features should be validated");
    assert_eq!(
        additional_features_error,
        "--additional-features must be a JSON object"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_service_runs_compose_build_with_cache_override_and_no_cache() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n    build:\n      context: .\n",
    )
    .expect("compose file");
    let compose_path = root.join("compose.sh");
    let log = root.join("compose.log");
    let captured_override = root.join("build-override.yml");
    write_executable_script(
        &compose_path,
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> '{}'
last_file=
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-f" ]; then
    last_file="$2"
    shift 2
    continue
  fi
  shift
done
if [ -n "$last_file" ] && [ -f "$last_file" ]; then
  cp "$last_file" '{}'
fi
exit 0
"#,
            log.display(),
            captured_override.display()
        ),
    );
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    );

    let image = build_service(
        &resolved,
        &[
            "--docker-compose-path".to_string(),
            compose_path.display().to_string(),
            "--cache-from".to_string(),
            "example/cache:latest".to_string(),
            "--build-no-cache".to_string(),
        ],
    )
    .expect("compose build");

    assert_eq!(image, "alpine:3.20");
    let log = fs::read_to_string(log).expect("compose log");
    assert!(log.contains("build --pull --no-cache app"), "{log}");
    let override_content = fs::read_to_string(captured_override).expect("override copy");
    assert!(
        override_content.contains("cache_from:"),
        "{override_content}"
    );
    assert!(
        override_content.contains("example/cache:latest"),
        "{override_content}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_service_returns_default_image_when_service_has_no_image_or_build() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(root.join("docker-compose.yml"), "services:\n  app: {}\n").expect("compose file");
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    );

    let image = build_service(&resolved, &[]).expect("default image");

    assert_eq!(
        image,
        format!(
            "{}-app",
            root.file_name().unwrap().to_string_lossy().to_lowercase()
        )
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn up_service_passes_no_recreate_and_configured_run_services() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n  db:\n    image: postgres:16\n",
    )
    .expect("compose file");
    let compose_path = root.join("compose.sh");
    let log_path = root.join("compose.log");
    write_executable_script(
        &compose_path,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            log_path.display()
        ),
    );
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "runServices": ["db"]
        }),
    );

    up_service(
        &resolved,
        &[
            "--docker-compose-path".to_string(),
            compose_path.display().to_string(),
        ],
        "/workspace",
        "alpine:3.20",
        true,
    )
    .expect("compose up");

    let log = fs::read_to_string(&log_path).expect("compose log");
    assert!(log.contains("up -d --no-recreate db app"), "{log}");

    fs::write(&log_path, "").expect("clear compose log");
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "runServices": ["app", "db"]
        }),
    );
    up_service(
        &resolved,
        &[
            "--docker-compose-path".to_string(),
            compose_path.display().to_string(),
        ],
        "/workspace",
        "alpine:3.20",
        false,
    )
    .expect("compose up with primary service listed");
    let log = fs::read_to_string(&log_path).expect("compose log");
    assert!(log.contains("up -d app db"), "{log}");
    assert!(!log.contains("up -d app db app"), "{log}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn resolve_container_id_can_include_stopped_and_skip_invalid_rows() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose file");
    let engine_path = root.join("engine.sh");
    write_executable_script(
        &engine_path,
        "#!/bin/sh\nif [ \"${1:-}\" = ps ]; then\n  printf '\\ninvalid id\\nabc123\\n'\nfi\nexit 0\n",
    );
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    );

    let container_id = resolve_container_id_including_stopped(
        &resolved,
        &[
            "--docker-path".to_string(),
            engine_path.display().to_string(),
        ],
    )
    .expect("container lookup");

    assert_eq!(container_id.as_deref(), Some("abc123"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_service_reports_compose_build_failures() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n    build:\n      context: .\n",
    )
    .expect("compose file");
    let compose_path = root.join("compose.sh");
    write_executable_script(
        &compose_path,
        "#!/bin/sh\ncase \" $* \" in\n  *\" build \"*) echo compose build failed >&2; exit 12 ;;\n  *) exit 0 ;;\nesac\n",
    );
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    );

    let error = build_service(
        &resolved,
        &[
            "--docker-compose-path".to_string(),
            compose_path.display().to_string(),
        ],
    )
    .expect_err("compose build failure");

    assert_eq!(error, "compose build failed");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_service_reports_feature_build_failures() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose file");
    let feature_dir = root.join("local-feature");
    fs::create_dir_all(&feature_dir).expect("feature dir");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        r#"{
  "id": "local-feature",
  "version": "1.0.0",
  "name": "Local Feature"
}
"#,
    )
    .expect("feature manifest");
    fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nexit 0\n").expect("install script");
    let engine_path = root.join("engine.sh");
    write_executable_script(
        &engine_path,
        "#!/bin/sh\nif [ \"${1:-}\" = build ]; then\n  echo feature build failed >&2\n  exit 17\nfi\nexit 0\n",
    );
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "features": {
                "./local-feature": {}
            }
        }),
    );

    let error = build_service(
        &resolved,
        &[
            "--docker-path".to_string(),
            engine_path.display().to_string(),
        ],
    )
    .expect_err("feature build failure");

    assert_eq!(error, "feature build failed");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_service_reports_lockfile_update_failures_after_feature_build() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace root");
    let compose_file = workspace.join("docker-compose.yml");
    fs::write(&compose_file, "services:\n  app:\n    image: alpine:3.20\n").expect("compose file");
    let feature_dir = workspace.join("local-feature");
    fs::create_dir_all(&feature_dir).expect("feature dir");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        r#"{
  "id": "local-feature",
  "version": "1.0.0",
  "name": "Local Feature"
}

"#,
    )
    .expect("feature manifest");
    fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nexit 0\n").expect("install script");
    let engine_path = root.join("engine.sh");
    write_executable_script(&engine_path, "#!/bin/sh\nexit 0\n");
    let missing_config_dir = root.join("missing-config-dir");
    let resolved = resolved_config(
        workspace.clone(),
        json!({
            "dockerComposeFile": compose_file.display().to_string(),
            "service": "app",
            "features": {
                (feature_dir.display().to_string()): {}
            }
        }),
    );
    let resolved = ResolvedConfig {
        config_file: missing_config_dir.join("devcontainer.json"),
        ..resolved
    };

    let error = build_service(
        &resolved,
        &[
            "--docker-path".to_string(),
            engine_path.display().to_string(),
        ],
    )
    .expect_err("lockfile update failure");

    assert!(!error.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn build_service_builds_feature_image_successfully() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose file");
    let feature_dir = root.join("local-feature");
    fs::create_dir_all(&feature_dir).expect("feature dir");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        r#"{
  "id": "local-feature",
  "version": "1.0.0",
  "name": "Local Feature"
}
"#,
    )
    .expect("feature manifest");
    fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nexit 0\n").expect("install script");
    let engine_path = root.join("engine.sh");
    let log = root.join("engine.log");
    write_executable_script(
        &engine_path,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            log.display()
        ),
    );
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "features": {
                "./local-feature": {}
            }
        }),
    );

    let image = build_service(
        &resolved,
        &[
            "--docker-path".to_string(),
            engine_path.display().to_string(),
            "--image-name".to_string(),
            "example/compose-featured:test".to_string(),
        ],
    )
    .expect("feature image");

    assert_eq!(image, "example/compose-featured:test");
    let log = fs::read_to_string(log).expect("engine log");
    assert!(
        log.contains("build --tag example/compose-featured:test"),
        "{log}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn up_service_pins_rebuilt_images_and_reports_compose_errors() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose file");
    let compose_path = root.join("compose.sh");
    write_executable_script(
        &compose_path,
        "#!/bin/sh\ncase \" $* \" in\n  *\" up \"*) echo compose up failed >&2; exit 19 ;;\n  *) exit 0 ;;\nesac\n",
    );
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    );

    let error = up_service(
        &resolved,
        &[
            "--docker-compose-path".to_string(),
            compose_path.display().to_string(),
        ],
        "/workspace",
        "example/rebuilt:latest",
        false,
    )
    .expect_err("compose up failure");

    assert_eq!(error, "compose up failed");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn up_service_reports_metadata_override_validation_errors() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose file");
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    );

    let error = up_service(
        &resolved,
        &[
            "--mount".to_string(),
            "type=bind,target=/workspace".to_string(),
        ],
        "/workspace",
        "alpine:3.20",
        false,
    )
    .expect_err("invalid mount should fail before compose up");

    assert!(
        error.contains("Invalid value for option --mount"),
        "{error}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn up_service_reports_missing_compose_binary() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose file");
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    );

    let error = up_service(
        &resolved,
        &[
            "--docker-compose-path".to_string(),
            root.join("missing-compose").display().to_string(),
        ],
        "/workspace",
        "alpine:3.20",
        false,
    )
    .expect_err("missing compose binary");

    assert!(
        error.contains("Container compose executable not found"),
        "{error}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn resolve_container_id_reports_engine_ps_failures() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose file");
    let engine_path = root.join("engine.sh");
    write_executable_script(
        &engine_path,
        "#!/bin/sh\nif [ \"${1:-}\" = ps ]; then\n  echo ps failed >&2\n  exit 23\nfi\nexit 0\n",
    );
    let resolved = resolved_config(
        root.clone(),
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    );

    let error = resolve_container_id(
        &resolved,
        &[
            "--docker-path".to_string(),
            engine_path.display().to_string(),
        ],
    )
    .expect_err("ps failure");

    assert_eq!(error, "ps failed");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn compose_feature_build_enforces_frozen_lockfile() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: alpine:3.20\n",
    )
    .expect("compose file");
    let feature_dir = root.join("local-feature");
    fs::create_dir_all(&feature_dir).expect("feature dir");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        r#"{
  "id": "local-feature",
  "version": "1.0.0",
  "name": "Local Feature"
}
"#,
    )
    .expect("feature manifest");
    fs::write(feature_dir.join("install.sh"), "#!/bin/sh\nexit 0\n").expect("install script");
    let engine_path = root.join("fake-docker.sh");
    write_executable_script(&engine_path, "#!/bin/sh\nexit 0\n");

    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "features": {
                "./local-feature": {}
            }
        }),
    };

    let error = build_service(
        &resolved,
        &[
            "--docker-path".to_string(),
            engine_path.display().to_string(),
            "--experimental-frozen-lockfile".to_string(),
        ],
    )
    .expect_err("expected frozen lockfile enforcement");

    assert_eq!(error, "Lockfile does not exist.");

    let _ = fs::remove_dir_all(root);
}
