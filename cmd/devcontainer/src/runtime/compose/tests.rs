//! Unit tests for compose runtime helpers.

use serde_json::json;
use std::fs;

use super::args::reject_unsupported_build_options;
use super::override_file::compose_metadata_override_file;
use super::project::{
    compose_name_from_file, compose_project_name, sanitize_project_name,
    substitute_compose_env_with,
};
use super::service::{
    compose_image_name_separator, inspect_service_definition, parse_semver_prefix,
};
use super::uses_compose_config;
use super::{build_service, load_compose_spec, resolve_container_id, up_service};
use crate::test_support::{init_git_repo, run_git, unique_temp_dir, write_executable_script};

fn compose_resolved(
    root: &std::path::Path,
    configuration: serde_json::Value,
) -> crate::runtime::context::ResolvedConfig {
    crate::runtime::context::ResolvedConfig {
        workspace_folder: root.to_path_buf(),
        config_file: root.join(".devcontainer.json"),
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
fn metadata_override_file_mounts_workspace_by_default() {
    let root = unique_temp_dir("devcontainer-compose-test");
    fs::create_dir_all(&root).expect("workspace root");
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: root.clone(),
        config_file: root.join(".devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
        }),
    };

    let override_file = compose_metadata_override_file(&resolved, &[], "/workspaces/project", None)
        .expect("override result")
        .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");
    let expected_mount_target = format!(
        "/workspaces/{}",
        root.file_name().unwrap().to_string_lossy()
    );

    assert!(override_content.contains("volumes:"));
    assert!(override_content.contains(&format!(
        "- '{}:{}'",
        root.display(),
        expected_mount_target
    )));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_mounts_nested_workspaces_from_the_git_root() {
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

    assert!(override_content.contains(&format!(
        "- '{}:/workspaces/repo'",
        expected_repo_root.display()
    )));
    assert!(!override_content.contains(&format!(
        "{}:/workspaces/repo/packages/app",
        expected_repo_root.display()
    )));

    let _ = fs::remove_file(override_file);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn metadata_override_file_rebases_worktree_common_dir_for_configured_workspace_folder() {
    let root = unique_temp_dir("devcontainer-compose-test");
    let repo_root = root.join("repo");
    let worktree_root = root.join("worktrees").join("feature");
    fs::create_dir_all(&repo_root).expect("repo root");
    init_git_repo(&repo_root);
    fs::write(repo_root.join("README.md"), "hello\n").expect("readme");
    run_git(&repo_root, &["add", "README.md"]);
    run_git(
        &repo_root,
        &[
            "-c",
            "user.name=Devcontainer Tests",
            "-c",
            "user.email=devcontainer-tests@example.com",
            "commit",
            "-m",
            "init",
            "--quiet",
        ],
    );
    if let Some(parent) = worktree_root.parent() {
        fs::create_dir_all(parent).expect("worktree parent");
    }
    run_git(
        &repo_root,
        &[
            "worktree",
            "add",
            "--relative-paths",
            worktree_root.to_string_lossy().as_ref(),
            "-b",
            "feature",
        ],
    );
    let expected_worktree_root = worktree_root
        .canonicalize()
        .unwrap_or_else(|_| worktree_root.clone());
    let expected_repo_git_dir = repo_root
        .join(".git")
        .canonicalize()
        .unwrap_or_else(|_| repo_root.join(".git"));
    let resolved = crate::runtime::context::ResolvedConfig {
        workspace_folder: expected_worktree_root.clone(),
        config_file: expected_worktree_root.join(".devcontainer.json"),
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

    assert!(override_content.contains(&format!(
        "- '{}:/workspace'",
        expected_worktree_root.display()
    )));
    assert!(override_content.contains(&expected_repo_git_dir.display().to_string()));
    assert!(override_content.contains("/repo/.git"));

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
fn metadata_override_file_preserves_workspace_mount_options() {
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

    let override_file = compose_metadata_override_file(&resolved, &[], "/workspaces/project", None)
        .expect("override result")
        .expect("override path");
    let override_content = fs::read_to_string(&override_file).expect("override content");

    assert!(override_content.contains("type: 'bind'"));
    assert!(override_content.contains("source: '/tmp/workspace'"));
    assert!(override_content.contains("target: '/workspaces/project'"));
    assert!(override_content.contains("consistency: 'delegated'"));
    assert!(!override_content.contains("/tmp/workspace:/workspaces/project"));

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

#[test]
fn build_service_runs_compose_build_with_cache_from_override_and_reports_failure() {
    let root = unique_temp_dir("devcontainer-compose-build-test");
    fs::create_dir_all(root.join(".devcontainer")).expect("config dir");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: example/app:base\n    build:\n      context: .\n      dockerfile: Dockerfile\n",
    )
    .expect("compose file");
    fs::write(root.join("Dockerfile"), "FROM alpine:3.20\n").expect("dockerfile");
    let fake_compose = root.join("compose");
    let log = root.join("compose.log");
    write_executable_script(
        &fake_compose,
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> '{}'
if [ -f "$(dirname "$0")/compose-fails" ]; then
  echo "compose build failed" >&2
  exit 8
fi
exit 0
"#,
            log.display()
        ),
    );
    let resolved = compose_resolved(
        &root,
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    );
    let args = vec![
        "--docker-compose-path".to_string(),
        fake_compose.display().to_string(),
        "--cache-from".to_string(),
        "type=registry,ref=example/cache:old".to_string(),
        "--build-no-cache".to_string(),
    ];

    let image = build_service(&resolved, &args).expect("compose build");
    fs::write(root.join("compose-fails"), "").expect("failure flag");
    let error = build_service(&resolved, &args).expect_err("compose build failure");
    let invocations = fs::read_to_string(log).expect("compose log");

    assert_eq!(image, "example/app:base");
    assert!(invocations.contains(" build --pull --no-cache app"));
    assert!(error.contains("compose build failed"), "{error}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn up_service_respects_run_services_no_recreate_and_reports_compose_failure() {
    let root = unique_temp_dir("devcontainer-compose-up-test");
    fs::create_dir_all(root.join(".devcontainer")).expect("config dir");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: example/app:base\n  db:\n    image: postgres:16\n",
    )
    .expect("compose file");
    let fake_compose = root.join("compose");
    let log = root.join("compose.log");
    write_executable_script(
        &fake_compose,
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> '{}'
echo "compose up failed" >&2
exit 4
"#,
            log.display()
        ),
    );
    let resolved = compose_resolved(
        &root,
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
            "runServices": ["db"]
        }),
    );

    let error = up_service(
        &resolved,
        &[
            "--docker-compose-path".to_string(),
            fake_compose.display().to_string(),
        ],
        "/workspace",
        "example/app:override",
        true,
    )
    .expect_err("compose up failure");
    let invocations = fs::read_to_string(log).expect("compose log");

    assert!(error.contains("compose up failed"), "{error}");
    assert!(invocations.contains(" up -d --no-recreate db app"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn resolve_container_id_filters_project_and_service_and_ignores_invalid_ids() {
    let root = unique_temp_dir("devcontainer-compose-ps-test");
    fs::create_dir_all(root.join(".devcontainer")).expect("config dir");
    fs::write(
        root.join("docker-compose.yml"),
        "name: Filter_Project\nservices:\n  app:\n    image: example/app:base\n",
    )
    .expect("compose file");
    let fake_engine = root.join("docker");
    let log = root.join("engine.log");
    write_executable_script(
        &fake_engine,
        &format!(
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> '{}'
if [ "$1" = ps ]; then
  printf '\ninvalid id\ncontainer-123\n'
  exit 0
fi
exit 2
"#,
            log.display()
        ),
    );
    let resolved = compose_resolved(
        &root,
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    );

    let container_id = resolve_container_id(
        &resolved,
        &[
            "--docker-path".to_string(),
            fake_engine.display().to_string(),
        ],
    )
    .expect("resolve")
    .expect("container");
    let invocation = fs::read_to_string(log).expect("engine log");

    assert_eq!(container_id, "container-123");
    assert!(invocation.contains("label=com.docker.compose.project=filter_project"));
    assert!(invocation.contains("label=com.docker.compose.service=app"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn reject_unsupported_build_options_rejects_label_cache_output_platform_push() {
    assert_eq!(
        reject_unsupported_build_options(&["--label".to_string(), "a=b".to_string()])
            .expect_err("label"),
        "--label not supported for compose builds."
    );
    assert_eq!(
        reject_unsupported_build_options(&["--cache-to".to_string(), "type=inline".to_string()])
            .expect_err("cache-to"),
        "--cache-to not supported for compose builds."
    );
    assert_eq!(
        reject_unsupported_build_options(&["--output".to_string(), "type=local".to_string()])
            .expect_err("output"),
        "--output not supported.".to_string()
    );
    assert_eq!(
        reject_unsupported_build_options(&["--platform".to_string(), "linux/amd64".to_string()])
            .expect_err("platform"),
        "--platform or --push not supported."
    );
    assert_eq!(
        reject_unsupported_build_options(&["--push".to_string()]).expect_err("push"),
        "--platform or --push not supported."
    );
}

#[test]
fn load_compose_spec_reports_missing_service_and_reads_user_image_build() {
    let root = unique_temp_dir("devcontainer-compose-spec-test");
    fs::create_dir_all(root.join(".devcontainer")).expect("config dir");
    fs::write(
        root.join("docker-compose.yml"),
        "services:\n  app:\n    image: example/app:base\n    user: vscode\n    build:\n      context: .\n",
    )
    .expect("compose file");
    let missing = compose_resolved(
        &root,
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "missing"
        }),
    );
    let resolved = compose_resolved(
        &root,
        json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app"
        }),
    );

    let error = load_compose_spec(&missing).err().expect("missing service");
    let spec = load_compose_spec(&resolved)
        .expect("load spec")
        .expect("compose spec");

    assert!(
        error.contains("Unable to locate compose service `missing`"),
        "{error}"
    );
    assert_eq!(spec.service, "app");
    assert_eq!(spec.image.as_deref(), Some("example/app:base"));
    assert_eq!(spec.user.as_deref(), Some("vscode"));
    assert!(spec.has_build);
    let _ = fs::remove_dir_all(root);
}
