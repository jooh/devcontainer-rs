//! Unit tests for compose runtime helpers.

use serde_json::json;
use std::fs;

use super::build_service;
use super::override_file::compose_metadata_override_file;
use super::project::{
    compose_name_from_file, compose_project_name, sanitize_project_name, substitute_compose_env,
};
use super::service::{
    compose_image_name_separator, inspect_service_definition, parse_semver_prefix,
};
use super::uses_compose_config;
use crate::test_support::{init_git_repo, run_git, unique_temp_dir, write_executable_script};

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
    let variable = format!("DEVCONTAINER_COMPOSE_TEST_PRESENT_{}", std::process::id());
    unsafe {
        std::env::set_var(&variable, "MyProject");
    }

    assert_eq!(
        substitute_compose_env(&format!("prefix-${variable}")),
        "prefix-MyProject"
    );

    unsafe {
        std::env::remove_var(variable);
    }
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
