//! Unit tests for UID-update image preparation helpers.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::runtime::context::ResolvedConfig;

use super::{
    command_stdout, prepare_up_image_for_platform, should_update_remote_user_uid,
    uid_update_details, uid_update_local_image_name, uid_update_run_args_user,
    unique_uid_update_build_context,
};

#[test]
fn remote_user_uid_update_defaults_to_on_for_supported_platforms() {
    assert!(should_update_remote_user_uid(
        &json!({
            "remoteUser": "vscode"
        }),
        &[],
        true,
    ));
}

#[test]
fn command_stdout_reports_non_success_stderr() {
    let error = command_stdout("sh", &["-c", "printf uid-failed >&2; exit 7"])
        .expect_err("command failure");

    assert_eq!(error, "uid-failed");
}

#[test]
fn remote_user_uid_update_can_inspect_image_user_when_config_omits_users() {
    assert!(should_update_remote_user_uid(&json!({}), &[], true));
}

#[test]
fn remote_user_uid_update_respects_option_and_config_overrides() {
    assert!(!should_update_remote_user_uid(
        &json!({
            "remoteUser": "vscode"
        }),
        &[
            "--update-remote-user-uid-default".to_string(),
            "off".to_string(),
        ],
        true,
    ));
    assert!(!should_update_remote_user_uid(
        &json!({
            "remoteUser": "vscode",
            "updateRemoteUserUID": false
        }),
        &[],
        true,
    ));
    assert!(!should_update_remote_user_uid(
        &json!({
            "remoteUser": "vscode"
        }),
        &[],
        false,
    ));
}

#[test]
fn uid_update_run_args_user_handles_missing_and_non_string_entries() {
    assert_eq!(
        uid_update_run_args_user(&json!({
            "runArgs": [42, "--user", "node", "-u=dev", "--user"]
        }))
        .as_deref(),
        Some("dev")
    );
    assert_eq!(
        uid_update_run_args_user(&json!({
            "runArgs": ["--user=app"]
        }))
        .as_deref(),
        Some("app")
    );
    assert!(uid_update_run_args_user(&json!({
        "runArgs": ["--user"]
    }))
    .is_none());
}

#[test]
fn prepare_up_image_skips_when_platform_unsupported_default_never_or_config_false() {
    let workspace = unique_uid_update_build_context();
    fs::create_dir_all(&workspace).expect("workspace");
    let unsupported = resolved_config(
        json!({
            "remoteUser": "vscode"
        }),
        &workspace,
    );
    let never = resolved_config(
        json!({
            "remoteUser": "vscode"
        }),
        &workspace,
    );
    let disabled = resolved_config(
        json!({
            "remoteUser": "vscode",
            "updateRemoteUserUID": false
        }),
        &workspace,
    );

    assert_eq!(
        prepare_up_image_for_platform(&unsupported, &[], "alpine:3.20", false)
            .expect("unsupported platform"),
        "alpine:3.20"
    );
    assert_eq!(
        prepare_up_image_for_platform(
            &never,
            &[
                "--update-remote-user-uid-default".to_string(),
                "never".to_string()
            ],
            "alpine:3.20",
            true,
        )
        .expect("default never"),
        "alpine:3.20"
    );
    assert_eq!(
        prepare_up_image_for_platform(&disabled, &[], "alpine:3.20", true)
            .expect("config disabled"),
        "alpine:3.20"
    );
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn prepare_up_image_skips_root_and_numeric_configured_users() {
    let workspace = unique_uid_update_build_context();
    fs::create_dir_all(&workspace).expect("workspace");
    let root_user = resolved_config(
        json!({
            "remoteUser": "root"
        }),
        &workspace,
    );
    let numeric_user = resolved_config(
        json!({
            "containerUser": "1000"
        }),
        &workspace,
    );

    assert_eq!(
        prepare_up_image_for_platform(&root_user, &[], "alpine:3.20", true).expect("root user"),
        "alpine:3.20"
    );
    assert_eq!(
        prepare_up_image_for_platform(&numeric_user, &[], "alpine:3.20", true)
            .expect("numeric user"),
        "alpine:3.20"
    );
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn uid_update_details_fall_back_to_the_image_user() {
    let details = uid_update_details(
        &json!({}),
        Path::new("/tmp/example-workspace"),
        "ghcr.io/example/app:latest",
        "node",
        None,
        Some("linux/amd64"),
    )
    .expect("uid update details");

    assert_eq!(details.remote_user, "node");
    assert_eq!(details.image_user, "node");
    assert_eq!(details.platform.as_deref(), Some("linux/amd64"));
    assert!(details
        .updated_image_name
        .starts_with("vsc-example-workspace-"));
    assert!(details.updated_image_name.ends_with("-uid"));
}

#[test]
fn uid_update_details_preserve_the_image_user_when_remote_user_is_overridden() {
    let details = uid_update_details(
        &json!({
            "remoteUser": "vscode"
        }),
        Path::new("/tmp/example-workspace"),
        "ghcr.io/example/app:latest",
        "node",
        None,
        None,
    )
    .expect("uid update details");

    assert_eq!(details.remote_user, "vscode");
    assert_eq!(details.image_user, "node");
}

#[test]
fn uid_update_details_use_a_local_tag_for_digest_pinned_images() {
    let details = uid_update_details(
        &json!({
            "remoteUser": "vscode"
        }),
        Path::new("/tmp/example-workspace"),
        "ghcr.io/example/app@sha256:0123456789abcdef",
        "node",
        None,
        None,
    )
    .expect("uid update details");

    assert!(details
        .updated_image_name
        .starts_with("vsc-example-workspace-"));
    assert!(details.updated_image_name.ends_with("-uid"));
    assert!(!details.updated_image_name.contains('@'));
}

#[test]
fn prepare_up_image_pulls_missing_image_before_uid_update() {
    let fixture = FakeEngineFixture::new();
    fixture.write("image-inspect.exit", "1\n");
    fixture.write(
        "image-inspect.stderr",
        "Error: No such image: ghcr.io/example/app:latest\n",
    );
    fixture.write(
        "image-inspect-after-pull.stdout",
        &image_inspect_output("root", Some("linux/amd64")),
    );

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "image": "ghcr.io/example/app:latest",
            "remoteUser": "vscode",
        }),
        &workspace,
    );

    let updated_image = prepare_up_image_for_platform(
        &resolved,
        &fixture.args(),
        "ghcr.io/example/app:latest",
        true,
    )
    .expect("prepare up image");

    assert_ne!(updated_image, "ghcr.io/example/app:latest");
    assert!(updated_image.ends_with("-uid"));
    let invocations = fixture.invocations();
    assert!(invocations.contains(&format!(
        "image inspect --format {} ghcr.io/example/app:latest",
        super::UID_UPDATE_IMAGE_INSPECT_FORMAT
    )));
    assert!(invocations.contains("pull ghcr.io/example/app:latest"));
    assert!(invocations.contains("build "));
    assert!(invocations.contains("--build-arg REMOTE_USER=vscode"));
    assert!(invocations.contains("--build-arg IMAGE_USER=root"));
}

#[test]
fn prepare_up_image_returns_original_when_image_inspect_missing_for_non_image_config() {
    let fixture = FakeEngineFixture::new();
    fixture.write("image-inspect.exit", "1\n");
    fixture.write(
        "image-inspect.stderr",
        "Error: No such image: local-build\n",
    );
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "build": {
                "dockerfile": "Dockerfile"
            },
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let image = prepare_up_image_for_platform(&resolved, &fixture.args(), "local-build", true)
        .expect("missing build image");

    assert_eq!(image, "local-build");
    assert!(!fixture.invocations().contains("pull local-build"));
}

#[test]
fn prepare_up_image_reports_pull_failure() {
    let fixture = FakeEngineFixture::new();
    fixture.write("image-inspect.exit", "1\n");
    fixture.write(
        "image-inspect.stderr",
        "Error: image not known: ghcr.io/example/app:latest\n",
    );
    fixture.write("pull.exit", "5\n");
    fixture.write("pull.stderr", "pull failed\n");
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "image": "ghcr.io/example/app:latest",
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let error = prepare_up_image_for_platform(
        &resolved,
        &fixture.args(),
        "ghcr.io/example/app:latest",
        true,
    )
    .expect_err("pull failure");

    assert!(error.contains("pull failed"), "{error}");
}

#[test]
fn prepare_up_image_reports_image_inspect_failure() {
    let fixture = FakeEngineFixture::new();
    fixture.write("image-inspect.exit", "6\n");
    fixture.write("image-inspect.stderr", "inspect failed\n");
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let error = prepare_up_image_for_platform(
        &resolved,
        &fixture.args(),
        "ghcr.io/example/app:latest",
        true,
    )
    .expect_err("inspect failure");

    assert!(error.contains("inspect failed"), "{error}");
}

#[test]
fn prepare_up_image_reports_build_failure() {
    let fixture = FakeEngineFixture::new();
    fixture.write(
        "image-inspect.stdout",
        &image_inspect_output("node", Some("linux/amd64")),
    );
    fixture.write("build.exit", "7\n");
    fixture.write("build.stderr", "uid build failed\n");
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let error = prepare_up_image_for_platform(
        &resolved,
        &fixture.args(),
        "ghcr.io/example/app:latest",
        true,
    )
    .expect_err("build failure");

    assert!(error.contains("uid build failed"), "{error}");
}

#[test]
fn prepare_up_image_uses_run_args_user_for_uid_update_selection() {
    let fixture = FakeEngineFixture::new();
    fixture.write(
        "image-inspect.stdout",
        &image_inspect_output("root", Some("linux/amd64")),
    );

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "runArgs": ["--user", "vscode"]
        }),
        &workspace,
    );

    let updated_image =
        prepare_up_image_for_platform(&resolved, &fixture.args(), "alpine:3.20", true)
            .expect("prepare up image");

    assert_ne!(updated_image, "alpine:3.20");
    assert!(updated_image.ends_with("-uid"));
    let invocations = fixture.invocations();
    assert!(invocations.contains("build "));
    assert!(invocations.contains("--build-arg REMOTE_USER=vscode"));
    assert!(invocations.contains("--build-arg IMAGE_USER=root"));
}

#[test]
fn uid_update_run_args_user_uses_last_user_flag_and_equals_forms() {
    let fixture = FakeEngineFixture::new();
    fixture.write(
        "image-inspect.stdout",
        &image_inspect_output("root", Some("linux/amd64")),
    );
    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "runArgs": [
                "--user=first",
                "-u",
                "second",
                "-u=third",
                "--user",
                "vscode"
            ]
        }),
        &workspace,
    );

    let updated_image =
        prepare_up_image_for_platform(&resolved, &fixture.args(), "alpine:3.20", true)
            .expect("prepare up image");
    let invocations = fixture.invocations();

    assert!(updated_image.ends_with("-uid"));
    assert!(invocations.contains("--build-arg REMOTE_USER=vscode"));
}

#[test]
fn prepare_up_image_preserves_the_inspected_platform_for_uid_update_builds() {
    let fixture = FakeEngineFixture::new();
    fixture.write(
        "image-inspect.stdout",
        &image_inspect_output("node", Some("linux/arm64/v8")),
    );

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let updated_image = prepare_up_image_for_platform(
        &resolved,
        &fixture.args(),
        "ghcr.io/example/app:latest",
        true,
    )
    .expect("prepare up image");

    assert!(updated_image.ends_with("-uid"));
    let invocations = fixture.invocations();
    assert!(invocations.contains("build "));
    assert!(invocations.contains("--platform linux/arm64/v8"));
}

#[test]
fn prepare_up_image_prefixes_local_podman_base_images_with_localhost() {
    let fixture = FakeEngineFixture::new();
    fixture.write(
        "image-inspect.stdout",
        &image_inspect_output("node", Some("linux/amd64")),
    );

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let local_image_name = uid_update_local_image_name(&workspace);
    let updated_image = prepare_up_image_for_platform(
        &resolved,
        &fixture.args_with_podman_name(),
        &local_image_name,
        true,
    )
    .expect("prepare up image");

    assert_eq!(updated_image, format!("{local_image_name}-uid"));
    let invocations = fixture.invocations();
    assert!(invocations.contains("build "));
    assert!(invocations.contains(&format!(
        "--build-arg BASE_IMAGE=localhost/{local_image_name}"
    )));
}

#[test]
fn prepare_up_image_retries_image_inspect_without_variant_template_for_podman() {
    let fixture = FakeEngineFixture::new();
    fixture.write("image-inspect-with-variant.exit", "1\n");
    fixture.write(
        "image-inspect-with-variant.stderr",
        "Error: template: inspect:2:30: executing \"inspect\" at <.Variant>: can't evaluate field Variant in type interface {}\n",
    );
    fixture.write(
        "image-inspect-without-variant.stdout",
        &image_inspect_output("node", Some("linux/amd64")),
    );

    let workspace = fixture.root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    let resolved = resolved_config(
        json!({
            "remoteUser": "vscode"
        }),
        &workspace,
    );

    let updated_image = prepare_up_image_for_platform(
        &resolved,
        &fixture.args_with_podman_name(),
        "ghcr.io/example/app:latest",
        true,
    )
    .expect("prepare up image");

    assert!(updated_image.ends_with("-uid"));
    let invocations = fixture.invocations();
    assert!(invocations.contains(&format!(
        "image inspect --format {} ghcr.io/example/app:latest",
        super::UID_UPDATE_IMAGE_INSPECT_FORMAT
    )));
    assert!(invocations.contains(&format!(
        "image inspect --format {} ghcr.io/example/app:latest",
        super::UID_UPDATE_IMAGE_INSPECT_FORMAT_NO_VARIANT
    )));
}

#[test]
fn prepare_up_image_uses_compose_service_user_for_uid_update_selection() {
    let fixture = FakeEngineFixture::new();
    fixture.write(
        "image-inspect.stdout",
        &image_inspect_output("root", Some("linux/amd64")),
    );

    let workspace = fixture.root.join("workspace");
    let config_root = workspace.join(".devcontainer");
    fs::create_dir_all(&config_root).expect("config dir");
    fs::write(
        config_root.join("docker-compose.yml"),
        "services:\n  app:\n    image: ghcr.io/example/app:latest\n    user: vscode\n",
    )
    .expect("compose file");
    let resolved = ResolvedConfig {
        workspace_folder: workspace.clone(),
        config_file: config_root.join("devcontainer.json"),
        configuration: json!({
            "dockerComposeFile": "docker-compose.yml",
            "service": "app",
        }),
    };

    let updated_image = prepare_up_image_for_platform(
        &resolved,
        &fixture.args(),
        "ghcr.io/example/app:latest",
        true,
    )
    .expect("prepare up image");

    assert!(updated_image.ends_with("-uid"));
    let invocations = fixture.invocations();
    assert!(invocations.contains("build "));
    assert!(invocations.contains("--build-arg REMOTE_USER=vscode"));
}

fn resolved_config(configuration: serde_json::Value, workspace_folder: &Path) -> ResolvedConfig {
    ResolvedConfig {
        workspace_folder: workspace_folder.to_path_buf(),
        config_file: workspace_folder
            .join(".devcontainer")
            .join("devcontainer.json"),
        configuration,
    }
}

struct FakeEngineFixture {
    root: PathBuf,
    engine_path: PathBuf,
    podman_engine_path: PathBuf,
    invocation_log: PathBuf,
}

impl FakeEngineFixture {
    fn new() -> Self {
        let root = unique_uid_update_build_context();
        fs::create_dir_all(&root).expect("fixture root");
        let engine_path = root.join("fake-engine");
        let podman_engine_path = root.join("podman");
        let invocation_log = root.join("invocations.log");
        let script = r#"#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
LOG="$ROOT/invocations.log"
printf '%s %s\n' "$1" "$*" >> "$LOG"

COMMAND="$1"
shift || true
case "$COMMAND" in
  image)
    SUBCOMMAND="${1:-}"
    shift || true
            case "$SUBCOMMAND" in
              inspect)
                if [ "${*}" != "${*#*'.Variant'*}" ]; then
                  if [ -f "$ROOT/image-inspect-with-variant.stdout" ]; then
                    cat "$ROOT/image-inspect-with-variant.stdout"
                  fi
                  if [ -f "$ROOT/image-inspect-with-variant.stderr" ]; then
                    cat "$ROOT/image-inspect-with-variant.stderr" >&2
                  fi
                  if [ -f "$ROOT/image-inspect-with-variant.exit" ]; then
                    exit "$(tr -d '\n' < "$ROOT/image-inspect-with-variant.exit")"
                  fi
                else
                  if [ -f "$ROOT/image-inspect-without-variant.stdout" ]; then
                    cat "$ROOT/image-inspect-without-variant.stdout"
                  fi
                  if [ -f "$ROOT/image-inspect-without-variant.stderr" ]; then
                    cat "$ROOT/image-inspect-without-variant.stderr" >&2
                  fi
                  if [ -f "$ROOT/image-inspect-without-variant.exit" ]; then
                    exit "$(tr -d '\n' < "$ROOT/image-inspect-without-variant.exit")"
                  fi
                fi
                if [ -f "$ROOT/pulled" ]; then
                  if [ -f "$ROOT/image-inspect-after-pull.stdout" ]; then
                    cat "$ROOT/image-inspect-after-pull.stdout"
                  fi
                  if [ -f "$ROOT/image-inspect-after-pull.stderr" ]; then
                    cat "$ROOT/image-inspect-after-pull.stderr" >&2
                  fi
                  if [ -f "$ROOT/image-inspect-after-pull.exit" ]; then
                    exit "$(tr -d '\n' < "$ROOT/image-inspect-after-pull.exit")"
                  fi
                  exit 0
                fi
                if [ -f "$ROOT/image-inspect.stdout" ]; then
                  cat "$ROOT/image-inspect.stdout"
                fi
                if [ -f "$ROOT/image-inspect.stderr" ]; then
                  cat "$ROOT/image-inspect.stderr" >&2
        fi
        if [ -f "$ROOT/image-inspect.exit" ]; then
          exit "$(tr -d '\n' < "$ROOT/image-inspect.exit")"
        fi
        exit 0
        ;;
    esac
    ;;
  pull)
    touch "$ROOT/pulled"
    if [ -f "$ROOT/pull.stdout" ]; then
      cat "$ROOT/pull.stdout"
    fi
    if [ -f "$ROOT/pull.stderr" ]; then
      cat "$ROOT/pull.stderr" >&2
    fi
    if [ -f "$ROOT/pull.exit" ]; then
      exit "$(tr -d '\n' < "$ROOT/pull.exit")"
    fi
    exit 0
    ;;
  build)
    if [ -f "$ROOT/build.stdout" ]; then
      cat "$ROOT/build.stdout"
    fi
    if [ -f "$ROOT/build.stderr" ]; then
      cat "$ROOT/build.stderr" >&2
    fi
    if [ -f "$ROOT/build.exit" ]; then
      exit "$(tr -d '\n' < "$ROOT/build.exit")"
    fi
    exit 0
    ;;
esac

echo "unsupported fake engine command: $COMMAND $*" >&2
exit 1
"#;
        fs::write(&engine_path, script).expect("engine script");
        fs::write(&podman_engine_path, script).expect("podman engine script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = fs::metadata(&engine_path)
                .expect("engine metadata")
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&engine_path, permissions.clone()).expect("engine permissions");
            fs::set_permissions(&podman_engine_path, permissions)
                .expect("podman engine permissions");
        }

        Self {
            root,
            engine_path,
            podman_engine_path,
            invocation_log,
        }
    }

    fn write(&self, name: &str, contents: &str) {
        fs::write(self.root.join(name), contents).expect("fixture file");
    }

    fn args(&self) -> Vec<String> {
        vec![
            "--docker-path".to_string(),
            self.engine_path.display().to_string(),
        ]
    }

    fn args_with_podman_name(&self) -> Vec<String> {
        vec![
            "--docker-path".to_string(),
            self.podman_engine_path.display().to_string(),
        ]
    }

    fn invocations(&self) -> String {
        fs::read_to_string(&self.invocation_log).expect("invocations")
    }
}

impl Drop for FakeEngineFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn image_inspect_output(user: &str, platform: Option<&str>) -> String {
    format!("{user}\n{}\n", platform.unwrap_or_default())
}
