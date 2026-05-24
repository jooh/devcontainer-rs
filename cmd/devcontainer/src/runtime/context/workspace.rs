//! Workspace-derived runtime paths, mounts, and environment helpers.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::commands::common;
use crate::runtime::compose;

use super::{DerivedWorkspaceMount, ResolvedConfig};

pub(crate) fn remote_user(configuration: &Value) -> String {
    configured_user(configuration).unwrap_or("root").to_string()
}

pub(crate) fn configured_user(configuration: &Value) -> Option<&str> {
    configuration
        .get("remoteUser")
        .or_else(|| configuration.get("containerUser"))
        .and_then(Value::as_str)
}

pub(crate) fn combined_remote_env(
    args: &[String],
    configuration: Option<&Value>,
) -> Result<HashMap<String, String>, String> {
    let mut remote_env = configuration
        .and_then(|configuration| configuration.get("remoteEnv"))
        .and_then(Value::as_object)
        .map(|remote_env| {
            remote_env
                .iter()
                .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.into())))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    remote_env.extend(common::secrets_env(args)?);
    remote_env.extend(common::remote_env_overrides(args));
    Ok(remote_env)
}

pub(crate) fn remote_workspace_folder_for_args(
    resolved: &ResolvedConfig,
    args: &[String],
) -> String {
    if compose::uses_compose_config(&resolved.configuration)
        && resolved.configuration.get("workspaceFolder").is_none()
        && resolved.configuration.get("workspaceMount").is_none()
    {
        return "/".to_string();
    }

    if let Some(workspace_folder) = resolved
        .configuration
        .get("workspaceFolder")
        .and_then(Value::as_str)
    {
        return workspace_folder.to_string();
    }

    if let Some(workspace_folder) = resolved
        .configuration
        .get("workspaceMount")
        .and_then(Value::as_str)
        .and_then(crate::runtime::mounts::mount_option_target)
    {
        return workspace_folder;
    }

    derived_workspace_mount(&resolved.workspace_folder, args)
        .expect("derived workspace mount")
        .remote_workspace_folder
}

pub(crate) fn workspace_mount_for_args(
    resolved: &ResolvedConfig,
    remote_workspace_folder: &str,
    args: &[String],
) -> String {
    resolved
        .configuration
        .get("workspaceMount")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            default_workspace_mount(
                &resolved.workspace_folder,
                &resolved.configuration,
                remote_workspace_folder,
                args,
            )
        })
}

pub(crate) fn default_remote_workspace_folder(workspace_folder: Option<&Path>) -> String {
    let basename = workspace_folder
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("workspace");
    format!("/workspaces/{basename}")
}

pub(crate) fn derived_workspace_mount(
    workspace_folder: &Path,
    args: &[String],
) -> Option<DerivedWorkspaceMount> {
    let mount_git_root = common::parse_bool_option(args, "--mount-workspace-git-root", true);
    if !mount_git_root {
        let remote_workspace_folder = default_remote_workspace_folder(Some(workspace_folder));
        let container_mount_folder = remote_workspace_folder.clone();
        return Some(DerivedWorkspaceMount {
            host_mount_folder: workspace_folder.to_path_buf(),
            container_mount_folder,
            remote_workspace_folder,
            additional_mounts: Vec::new(),
        });
    }

    let host_mount_folder =
        find_git_root_folder(workspace_folder).unwrap_or_else(|| workspace_folder.to_path_buf());
    let mut container_mount_folder = format!(
        "/workspaces/{}",
        host_mount_folder
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("workspace")
    );
    let mut additional_mounts = Vec::new();
    if common::parse_bool_option(args, "--mount-git-worktree-common-dir", false) {
        if let Some((updated_container_mount_folder, additional_mount)) =
            git_worktree_common_dir_mount(&host_mount_folder, args, &container_mount_folder)
        {
            container_mount_folder = updated_container_mount_folder;
            additional_mounts.push(additional_mount);
        }
    }
    let relative_workspace_folder = workspace_folder
        .strip_prefix(&host_mount_folder)
        .unwrap_or_else(|_| Path::new(""));
    let remote_workspace_folder =
        join_container_path(&container_mount_folder, relative_workspace_folder);
    Some(DerivedWorkspaceMount {
        host_mount_folder,
        container_mount_folder,
        remote_workspace_folder,
        additional_mounts,
    })
}

pub(crate) fn additional_mounts_for_workspace_target(
    resolved: &ResolvedConfig,
    remote_workspace_folder: &str,
    args: &[String],
) -> Vec<String> {
    let derived =
        derived_workspace_mount(&resolved.workspace_folder, args).expect("derived workspace mount");
    if resolved.configuration.get("workspaceFolder").is_none() {
        return derived.additional_mounts;
    }

    let mut additional_mounts = Vec::new();
    if common::parse_bool_option(args, "--mount-git-worktree-common-dir", false) {
        if let Some(additional_mount) = git_worktree_common_dir_mount_for_workspace_target(
            &derived.host_mount_folder,
            args,
            remote_workspace_folder,
        ) {
            additional_mounts.push(additional_mount);
        }
    }
    additional_mounts
}

fn default_workspace_mount(
    workspace_folder: &Path,
    configuration: &Value,
    remote_workspace_folder: &str,
    args: &[String],
) -> String {
    let derived = derived_workspace_mount(workspace_folder, args).expect("derived workspace mount");
    if configuration
        .get("workspaceFolder")
        .and_then(Value::as_str)
        .is_some()
    {
        let mut mount = format!(
            "type=bind,source={},target={remote_workspace_folder}",
            derived.host_mount_folder.display()
        );
        append_workspace_mount_consistency(&mut mount, args);
        return mount;
    }
    let mut mount = format!(
        "type=bind,source={},target={}",
        derived.host_mount_folder.display(),
        derived.container_mount_folder
    );
    append_workspace_mount_consistency(&mut mount, args);
    mount
}

fn git_worktree_common_dir_mount(
    host_mount_folder: &Path,
    args: &[String],
    default_container_mount_folder: &str,
) -> Option<(String, String)> {
    let worktree_mount = git_worktree_common_dir_info(host_mount_folder)?;
    let container_mount_folder = if worktree_mount
        .relative_host_mount_folder
        .components()
        .next()
        .is_none()
    {
        default_container_mount_folder.to_string()
    } else {
        join_container_path("/workspaces", &worktree_mount.relative_host_mount_folder)
    };
    let container_git_common_dir =
        join_container_path("/workspaces", &worktree_mount.relative_git_common_dir);
    let mut additional_mount = format!(
        "type=bind,source={},target={container_git_common_dir}",
        worktree_mount.git_common_dir.display(),
    );
    append_workspace_mount_consistency(&mut additional_mount, args);

    Some((container_mount_folder, additional_mount))
}

fn git_worktree_common_dir_mount_for_workspace_target(
    host_mount_folder: &Path,
    args: &[String],
    container_workspace_folder: &str,
) -> Option<String> {
    let worktree_mount = git_worktree_common_dir_info(host_mount_folder)?;
    let container_common_dir_base = ascend_container_path(
        container_workspace_folder,
        worktree_mount
            .relative_host_mount_folder
            .components()
            .count(),
    );
    let container_git_common_dir = join_container_path(
        &container_common_dir_base,
        &worktree_mount.relative_git_common_dir,
    );
    let mut additional_mount = format!(
        "type=bind,source={},target={container_git_common_dir}",
        worktree_mount.git_common_dir.display(),
    );
    append_workspace_mount_consistency(&mut additional_mount, args);

    Some(additional_mount)
}

struct GitWorktreeCommonDirInfo {
    git_common_dir: PathBuf,
    relative_host_mount_folder: PathBuf,
    relative_git_common_dir: PathBuf,
}

fn git_worktree_common_dir_info(host_mount_folder: &Path) -> Option<GitWorktreeCommonDirInfo> {
    let dot_git_path = host_mount_folder.join(".git");
    if !dot_git_path.is_file() {
        return None;
    }

    let dot_git_content = fs::read_to_string(&dot_git_path).ok()?;
    let gitdir = dot_git_content
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))
        .map(str::trim)?;
    let gitdir_path = Path::new(gitdir);
    if gitdir_path.is_absolute() {
        return None;
    }

    let git_common_dir = normalize_path(host_mount_folder.join(gitdir_path).join("..").join(".."));
    let mut current = host_mount_folder;
    while !git_common_dir.starts_with(current) {
        current = current.parent()?;
    }
    let relative_host_mount_folder = host_mount_folder.strip_prefix(current).ok()?.to_path_buf();
    let relative_git_common_dir = git_common_dir.strip_prefix(current).ok()?.to_path_buf();
    Some(GitWorktreeCommonDirInfo {
        git_common_dir,
        relative_host_mount_folder,
        relative_git_common_dir,
    })
}

fn normalize_path(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path)
        .ok()
        .or_else(|| path.exists().then_some(path.clone()))
        .unwrap_or_else(|| lexically_normalize_path(&path))
}

fn lexically_normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            if !normalized.pop() {
                normalized.push("..");
            }
        } else if !matches!(component, std::path::Component::CurDir) {
            normalized.push(component.as_os_str());
        }
    }
    normalized
}

fn join_container_path(base: &str, relative: &Path) -> String {
    relative
        .components()
        .fold(base.to_string(), |mut path, component| {
            if let std::path::Component::Normal(segment) = component {
                if !path.ends_with('/') {
                    path.push('/');
                }
                path.push_str(&segment.to_string_lossy());
            }
            path
        })
}

fn ascend_container_path(path: &str, segments: usize) -> String {
    let mut parts = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    for _ in 0..segments {
        if parts.pop().is_none() {
            return "/".to_string();
        }
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

fn append_workspace_mount_consistency(mount: &mut String, args: &[String]) {
    #[cfg(target_os = "linux")]
    {
        let _ = (mount, args);
    }
    #[cfg(not(target_os = "linux"))]
    if let Some(consistency) = common::parse_option_value(args, "--workspace-mount-consistency") {
        mount.push_str(&format!(",consistency={consistency}"));
    }
}

fn find_git_root_folder(workspace_folder: &Path) -> Option<PathBuf> {
    let git_output = Command::new("git")
        .args(["rev-parse", "--show-cdup"])
        .current_dir(workspace_folder)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let cdup = String::from_utf8_lossy(&git_output.stdout)
        .trim()
        .to_string();
    git_root_folder_from_cdup(workspace_folder, &cdup)
}

fn git_root_folder_from_cdup(workspace_folder: &Path, cdup: &str) -> Option<PathBuf> {
    if cdup.is_empty() {
        return Some(workspace_folder.to_path_buf());
    }
    let candidate = workspace_folder.join(cdup);
    fs::canonicalize(&candidate)
        .ok()
        .or_else(|| candidate.exists().then_some(candidate))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::{
        additional_mounts_for_workspace_target, ascend_container_path, combined_remote_env,
        configured_user, default_remote_workspace_folder, derived_workspace_mount,
        git_root_folder_from_cdup, git_worktree_common_dir_mount,
        git_worktree_common_dir_mount_for_workspace_target, join_container_path, normalize_path,
        remote_user, remote_workspace_folder_for_args, workspace_mount_for_args, ResolvedConfig,
    };
    use crate::test_support::unique_temp_dir;

    #[test]
    fn remote_user_prefers_remote_user_then_container_user_then_root() {
        assert_eq!(remote_user(&json!({ "remoteUser": "vscode" })), "vscode");
        assert_eq!(
            configured_user(&json!({ "containerUser": "node" })),
            Some("node")
        );
        assert_eq!(remote_user(&json!({})), "root");
    }

    #[test]
    fn combined_remote_env_merges_config_secrets_and_cli_overrides() {
        let root = unique_temp_dir("devcontainer-workspace-test");
        fs::create_dir_all(&root).expect("temp root");
        let secrets = root.join("secrets.json");
        fs::write(
            &secrets,
            json!({
                "SECRET": "from-file",
                "BOOL": true
            })
            .to_string(),
        )
        .expect("secrets");
        let args = vec![
            "--secrets-file".to_string(),
            secrets.to_string_lossy().to_string(),
            "--remote-env".to_string(),
            "CONFIG=from-cli".to_string(),
            "--remote-env".to_string(),
            "CLI=present".to_string(),
        ];

        let env = combined_remote_env(
            &args,
            Some(&json!({
                "remoteEnv": {
                    "CONFIG": "from-config",
                    "IGNORED": true
                }
            })),
        )
        .expect("remote env");

        assert_eq!(env.get("CONFIG").map(String::as_str), Some("from-cli"));
        assert_eq!(env.get("CLI").map(String::as_str), Some("present"));
        assert_eq!(env.get("SECRET").map(String::as_str), Some("from-file"));
        assert_eq!(env.get("BOOL").map(String::as_str), Some("true"));
        assert!(!env.contains_key("IGNORED"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn combined_remote_env_reports_invalid_secret_files() {
        let root = unique_temp_dir("devcontainer-workspace-test");
        fs::create_dir_all(&root).expect("temp root");
        let secrets = root.join("secrets.json");
        fs::write(&secrets, "not json").expect("secrets");
        let args = vec![
            "--secrets-file".to_string(),
            secrets.to_string_lossy().to_string(),
        ];

        let error = combined_remote_env(&args, Some(&json!({}))).expect_err("invalid secrets");

        assert!(error.contains("expected"), "{error}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_workspace_folder_uses_compose_root_when_no_workspace_is_configured() {
        let resolved = ResolvedConfig {
            workspace_folder: PathBuf::from("/tmp/example"),
            config_file: PathBuf::from("/tmp/example/.devcontainer/devcontainer.json"),
            configuration: json!({
                "dockerComposeFile": "docker-compose.yml",
                "service": "app"
            }),
        };

        assert_eq!(remote_workspace_folder_for_args(&resolved, &[]), "/");
    }

    #[test]
    fn workspace_mount_for_args_preserves_configured_mount() {
        let resolved = ResolvedConfig {
            workspace_folder: PathBuf::from("/tmp/example"),
            config_file: PathBuf::from("/tmp/example/.devcontainer/devcontainer.json"),
            configuration: json!({
                "workspaceMount": "type=bind,source=/tmp/example,target=/workspace"
            }),
        };

        assert_eq!(
            workspace_mount_for_args(&resolved, "/ignored", &[]),
            "type=bind,source=/tmp/example,target=/workspace"
        );
    }

    #[test]
    fn default_remote_workspace_folder_uses_generic_name_without_workspace() {
        assert_eq!(
            default_remote_workspace_folder(None),
            "/workspaces/workspace"
        );
    }

    #[test]
    fn normalize_path_collapses_parent_segments_without_existing_paths() {
        let root = unique_temp_dir("devcontainer-workspace-test");
        let unresolved = root
            .join("worktrees")
            .join("feature")
            .join("..")
            .join("..")
            .join("repo")
            .join(".git");

        assert_eq!(normalize_path(unresolved), root.join("repo").join(".git"));
    }

    #[test]
    fn normalize_path_collapses_current_dir_and_leading_parent_segments() {
        assert_eq!(
            normalize_path(PathBuf::from("../repo/./file")),
            PathBuf::from("../repo/file")
        );
        assert_eq!(
            normalize_path(PathBuf::from("/definitely/missing/../target")),
            PathBuf::from("/definitely/target")
        );
    }

    #[test]
    fn join_container_path_appends_only_normal_relative_segments() {
        assert_eq!(
            join_container_path("/workspaces", Path::new("../repo/./file")),
            "/workspaces/repo/file"
        );
    }

    #[test]
    fn git_worktree_common_dir_mount_normalizes_nonexistent_relative_gitdir_targets() {
        let root = unique_temp_dir("devcontainer-workspace-test");
        let worktree = root.join("worktrees").join("feature");
        fs::create_dir_all(&worktree).expect("worktree dir");
        fs::write(
            worktree.join(".git"),
            "gitdir: ../../repo/.git/worktrees/feature\n",
        )
        .expect("git file");

        let (_, additional_mount) =
            git_worktree_common_dir_mount(&worktree, &[], "/workspaces/feature")
                .expect("additional mount");

        assert_eq!(
            additional_mount,
            format!(
                "type=bind,source={},target=/workspaces/repo/.git",
                root.join("repo").join(".git").display()
            )
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn git_worktree_common_dir_mount_reuses_default_container_folder_for_empty_relative_host() {
        let root = unique_temp_dir("devcontainer-workspace-test");
        fs::create_dir_all(&root).expect("root dir");
        fs::write(root.join(".git"), "gitdir: .git/worktrees/main\n").expect("git file");

        let (container_mount_folder, additional_mount) =
            git_worktree_common_dir_mount(&root, &[], "/workspaces/root")
                .expect("additional mount");

        assert_eq!(container_mount_folder, "/workspaces/root");
        assert_eq!(
            additional_mount,
            format!(
                "type=bind,source={},target=/workspaces/.git",
                root.join(".git").display()
            )
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn git_worktree_common_dir_mount_rebases_common_dir_for_custom_workspace_target() {
        let root = unique_temp_dir("devcontainer-workspace-test");
        let worktree = root.join("worktrees").join("feature");
        fs::create_dir_all(&worktree).expect("worktree dir");
        fs::write(
            worktree.join(".git"),
            "gitdir: ../../repo/.git/worktrees/feature\n",
        )
        .expect("git file");

        let additional_mount =
            git_worktree_common_dir_mount_for_workspace_target(&worktree, &[], "/workspace")
                .expect("additional mount");

        assert_eq!(
            additional_mount,
            format!(
                "type=bind,source={},target=/repo/.git",
                root.join("repo").join(".git").display()
            )
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn additional_mounts_rebase_common_dir_for_custom_workspace_folder_targets() {
        let root = unique_temp_dir("devcontainer-workspace-test");
        let workspace = root.join("worktree");
        fs::create_dir_all(&workspace).expect("workspace dir");
        fs::write(
            workspace.join(".git"),
            "gitdir: ../repo/.git/worktrees/worktree\n",
        )
        .expect("git file");
        let resolved = ResolvedConfig {
            workspace_folder: workspace.clone(),
            config_file: workspace.join(".devcontainer").join("devcontainer.json"),
            configuration: json!({
                "workspaceFolder": "/workspace"
            }),
        };

        let additional_mounts = additional_mounts_for_workspace_target(
            &resolved,
            "/workspace",
            &[
                "--mount-git-worktree-common-dir".to_string(),
                "true".to_string(),
            ],
        );

        assert_eq!(
            additional_mounts,
            vec![format!(
                "type=bind,source={},target=/repo/.git",
                root.join("repo").join(".git").display()
            )]
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn additional_mounts_use_derived_mounts_without_configured_workspace_folder() {
        let root = unique_temp_dir("devcontainer-workspace-test");
        let workspace = root.join("worktree");
        fs::create_dir_all(&workspace).expect("workspace dir");
        fs::write(
            workspace.join(".git"),
            "gitdir: ../repo/.git/worktrees/worktree\n",
        )
        .expect("git file");
        let resolved = ResolvedConfig {
            workspace_folder: workspace.clone(),
            config_file: workspace.join(".devcontainer").join("devcontainer.json"),
            configuration: json!({}),
        };

        let additional_mounts = additional_mounts_for_workspace_target(
            &resolved,
            "/workspaces/worktree",
            &[
                "--mount-git-worktree-common-dir".to_string(),
                "true".to_string(),
            ],
        );

        assert_eq!(additional_mounts.len(), 1);
        assert!(additional_mounts[0].contains("target=/workspaces/repo/.git"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn additional_mounts_skip_common_dir_when_flag_is_disabled() {
        let resolved = ResolvedConfig {
            workspace_folder: PathBuf::from("/tmp/example"),
            config_file: PathBuf::from("/tmp/example/.devcontainer/devcontainer.json"),
            configuration: json!({
                "workspaceFolder": "/workspace"
            }),
        };

        assert!(additional_mounts_for_workspace_target(&resolved, "/workspace", &[]).is_empty());
    }

    #[test]
    fn common_dir_mounts_skip_when_flag_enabled_without_worktree_git_file() {
        let root = unique_temp_dir("devcontainer-workspace-test");
        fs::create_dir_all(&root).expect("root dir");
        let resolved = ResolvedConfig {
            workspace_folder: root.clone(),
            config_file: root.join(".devcontainer").join("devcontainer.json"),
            configuration: json!({
                "workspaceFolder": "/workspace"
            }),
        };
        let args = vec![
            "--mount-git-worktree-common-dir".to_string(),
            "true".to_string(),
        ];

        let derived = derived_workspace_mount(&root, &args).expect("derived mount");
        let additional = additional_mounts_for_workspace_target(&resolved, "/workspace", &args);

        assert!(derived.additional_mounts.is_empty());
        assert!(additional.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn git_worktree_common_dir_mount_skips_missing_dot_git_files() {
        let root = unique_temp_dir("devcontainer-workspace-test");
        fs::create_dir_all(&root).expect("root dir");

        assert!(git_worktree_common_dir_mount(&root, &[], "/workspaces/root").is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn git_worktree_common_dir_mount_skips_invalid_gitdir_files() {
        let root = unique_temp_dir("devcontainer-workspace-test");
        fs::create_dir_all(&root).expect("root dir");
        fs::write(root.join(".git"), "not a gitdir file\n").expect("git file");

        assert!(git_worktree_common_dir_mount(&root, &[], "/workspaces/root").is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn git_worktree_common_dir_mount_skips_absolute_gitdir_targets() {
        let root = unique_temp_dir("devcontainer-workspace-test");
        let worktree = root.join("worktrees").join("feature");
        fs::create_dir_all(&worktree).expect("worktree dir");
        fs::write(
            worktree.join(".git"),
            "gitdir: /absolute/repo/.git/worktrees/feature\n",
        )
        .expect("git file");

        assert!(git_worktree_common_dir_mount(&worktree, &[], "/workspaces/feature").is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ascend_container_path_clamps_at_root() {
        assert_eq!(ascend_container_path("/workspace", 2), "/");
        assert_eq!(ascend_container_path("/one/two/three", 2), "/one");
    }

    #[test]
    fn git_root_folder_from_cdup_handles_empty_existing_and_missing_roots() {
        let root = unique_temp_dir("devcontainer-workspace-test");
        let workspace = root.join("repo").join("packages").join("app");
        let repo = root.join("repo");
        fs::create_dir_all(&workspace).expect("workspace dir");
        let expected_repo = fs::canonicalize(&repo).expect("canonical repo root");

        assert_eq!(
            git_root_folder_from_cdup(&workspace, ""),
            Some(workspace.clone())
        );
        assert_eq!(
            git_root_folder_from_cdup(&workspace, "../.."),
            Some(expected_repo)
        );
        assert_eq!(git_root_folder_from_cdup(&workspace, "../../missing"), None);

        let _ = fs::remove_dir_all(root);
    }
}
