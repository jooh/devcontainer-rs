//! Unit tests for configuration upgrade and lockfile behavior.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;

use serde_json::json;
use sha2::{Digest, Sha256};

use super::support::unique_temp_dir;
use crate::commands::configuration::inspect::merged_configuration_payload;
use crate::commands::configuration::read::{
    build_read_configuration_payload, should_use_native_read_configuration,
};
use crate::commands::configuration::upgrade::{
    build_outdated_payload, feature_id_without_version, lockfile_for_resolution, lockfile_path,
    parse_feature_reference, render_outdated_text, run_upgrade_lockfile,
};
use crate::commands::configuration::{
    ensure_native_lockfile, feature_installation_name, materialize_feature_installation,
    resolve_feature_support, resolve_feature_support_without_lockfile, run_outdated, run_upgrade,
    validate_lockfile_options, validate_native_lockfile, warn_deprecated_lockfile_flags,
};
use crate::output::{render_log, CommandLogLevel, LogFormat};

#[test]
fn outdated_payload_reports_remote_feature_versions() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": \"latest\",\n    \"./local-feature\": {}\n  }\n}\n",
    )
    .expect("failed to write config");
    fs::write(
        root.join(".devcontainer-lock.json"),
        "{\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": {\n      \"version\": \"1.0.4\",\n      \"resolved\": \"ghcr.io/devcontainers/features/git@sha256:0bb490abcc0a3fb23937d29e2c18a225b51c5584edc0d9eb4131569a980f60b6\",\n      \"integrity\": \"sha256:0bb490abcc0a3fb23937d29e2c18a225b51c5584edc0d9eb4131569a980f60b6\"\n    }\n  }\n}\n",
    )
    .expect("failed to write lockfile");

    let args = vec!["--workspace-folder".to_string(), root.display().to_string()];
    let payload = build_outdated_payload(&args).expect("payload");

    assert_eq!(
        payload["features"]["ghcr.io/devcontainers/features/git:1.0"]["current"],
        "1.0.4"
    );
    assert_eq!(
        payload["features"]["ghcr.io/devcontainers/features/git:1.0"]["wanted"],
        "1.0.5"
    );
    assert_eq!(
        payload["features"]["ghcr.io/devcontainers/features/git:1.0"]["latest"],
        "1.2.0"
    );
    assert!(payload["features"]["./local-feature"].is_null());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_lockfile_uses_root_relative_lockfile_for_dotfile_configs() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/github-cli\": \"latest\"\n  }\n}\n",
    )
    .expect("failed to write config");

    let lockfile =
        run_upgrade_lockfile(&["--workspace-folder".to_string(), root.display().to_string()])
            .expect("lockfile payload");

    let lockfile_path = root.join(".devcontainer-lock.json");
    assert!(lockfile_path.is_file());
    assert_eq!(
        lockfile.features["ghcr.io/devcontainers/features/github-cli"].version,
        "1.0.9"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_lockfile_records_direct_tarball_archive_digest() {
    let _env_guard = crate::test_support::process_env_lock();
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let tarball_bytes = b"feature archive bytes used for lockfile integrity";
    let server = SingleResponseHttpServer::new(tarball_bytes);
    let feature_uri = server.url("devcontainer-feature-network.tgz");
    fs::write(
        root.join(".devcontainer.json"),
        format!(
            "{{\n  \"image\": \"debian:bookworm\",\n  \"features\": {{\n    \"{feature_uri}\": {{}}\n  }}\n}}\n"
        ),
    )
    .expect("failed to write config");

    let lockfile =
        run_upgrade_lockfile(&["--workspace-folder".to_string(), root.display().to_string()])
            .expect("lockfile payload");

    let entry = &lockfile.features[&feature_uri];
    assert_eq!(entry.resolved, feature_uri);
    assert_eq!(
        entry.integrity,
        format!("sha256:{}", sha256_digest(tarball_bytes))
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ensure_native_lockfile_rejects_changed_direct_tarball_archive() {
    let _env_guard = crate::test_support::process_env_lock();
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let old_tarball_bytes = b"original direct tarball archive bytes";
    let changed_tarball_bytes = b"changed direct tarball archive bytes";
    let server = SingleResponseHttpServer::new(changed_tarball_bytes);
    let feature_uri = server.url("devcontainer-feature-network.tgz");
    let configuration = json!({
        "image": "debian:bookworm",
        "features": {
            feature_uri.clone(): {},
        },
    });
    let config_file = root.join(".devcontainer.json");
    fs::write(
        &config_file,
        serde_json::to_string_pretty(&configuration).expect("config json"),
    )
    .expect("failed to write config");
    fs::write(
        root.join(".devcontainer-lock.json"),
        format!(
            "{{\n  \"features\": {{\n    \"{feature_uri}\": {{\n      \"version\": \"latest\",\n      \"resolved\": \"{feature_uri}\",\n      \"integrity\": \"sha256:{}\"\n    }}\n  }}\n}}\n",
            sha256_digest(old_tarball_bytes)
        ),
    )
    .expect("failed to write lockfile");

    let error = ensure_native_lockfile_for_config(&[], &config_file, &configuration)
        .expect_err("changed direct tarball should fail integrity verification");

    assert!(error.contains("Digest did not match"), "{error}");
    let lockfile = fs::read_to_string(root.join(".devcontainer-lock.json")).expect("lockfile");
    assert!(lockfile.contains(&sha256_digest(old_tarball_bytes)));
    assert!(!lockfile.contains(&sha256_digest(changed_tarball_bytes)));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn feature_id_without_version_handles_tags_and_digests() {
    assert_eq!(
        feature_id_without_version("ghcr.io/devcontainers/features/git:1.0"),
        "ghcr.io/devcontainers/features/git"
    );
    assert_eq!(
        feature_id_without_version(
            "ghcr.io/devcontainers/features/git-lfs@sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c"
        ),
        "ghcr.io/devcontainers/features/git-lfs"
    );
    assert_eq!(
        feature_id_without_version("ghcr.io/devcontainers/features/git@1"),
        "ghcr.io/devcontainers/features/git"
    );
    assert_eq!(
        feature_id_without_version("ghcr.io/devcontainers/features/git:1@beta"),
        "ghcr.io/devcontainers/features/git:1"
    );
}

#[test]
fn parse_feature_reference_handles_plain_and_digest_features() {
    let plain = parse_feature_reference("ghcr.io/devcontainers/features/git").expect("plain");
    assert_eq!(plain.base, "ghcr.io/devcontainers/features/git");
    assert!(plain.tag.is_none());
    assert!(plain.digest.is_none());

    let digest = parse_feature_reference("ghcr.io/devcontainers/features/git@sha256:abc123")
        .expect("digest");
    assert_eq!(digest.base, "ghcr.io/devcontainers/features/git");
    assert!(digest.tag.is_none());
    assert_eq!(digest.digest.as_deref(), Some("sha256:abc123"));
}

#[test]
fn lockfile_path_matches_upstream_dotfile_rule() {
    assert_eq!(
        lockfile_path(Path::new("/tmp/workspace/.devcontainer.json")),
        PathBuf::from("/tmp/workspace/.devcontainer-lock.json")
    );
    assert_eq!(
        lockfile_path(Path::new("/tmp/workspace/.devcontainer/devcontainer.json")),
        PathBuf::from("/tmp/workspace/.devcontainer/devcontainer-lock.json")
    );
}

#[test]
fn outdated_payload_reads_workspace_oci_layout_versions() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let digest = write_workspace_layout_version(
        &root,
        "ghcr.io/acme/features/published-feature",
        "1.0.0",
        None,
    );
    write_workspace_layout_version(
        &root,
        "ghcr.io/acme/features/published-feature",
        "1.0.1",
        None,
    );
    write_workspace_layout_version(
        &root,
        "ghcr.io/acme/features/published-feature",
        "2.0.0",
        None,
    );
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/acme/features/published-feature:1.0\": {}\n  }\n}\n",
    )
    .expect("failed to write config");
    fs::write(
        root.join(".devcontainer-lock.json"),
        format!(
            "{{\n  \"features\": {{\n    \"ghcr.io/acme/features/published-feature:1.0\": {{\n      \"version\": \"1.0.0\",\n      \"resolved\": \"ghcr.io/acme/features/published-feature@sha256:{digest}\",\n      \"integrity\": \"sha256:{digest}\"\n    }}\n  }}\n}}\n"
        ),
    )
    .expect("failed to write lockfile");

    let args = vec!["--workspace-folder".to_string(), root.display().to_string()];
    let payload = build_outdated_payload(&args).expect("payload");

    assert_eq!(
        payload["features"]["ghcr.io/acme/features/published-feature:1.0"]["current"],
        "1.0.0"
    );
    assert_eq!(
        payload["features"]["ghcr.io/acme/features/published-feature:1.0"]["wanted"],
        "1.0.1"
    );
    assert_eq!(
        payload["features"]["ghcr.io/acme/features/published-feature:1.0"]["latest"],
        "2.0.0"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_lockfile_reads_workspace_oci_layout_digests() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    write_workspace_layout_version(
        &root,
        "ghcr.io/acme/features/published-feature",
        "1.0.0",
        None,
    );
    let latest_digest = write_workspace_layout_version(
        &root,
        "ghcr.io/acme/features/published-feature",
        "1.1.0",
        Some(&["ghcr.io/acme/features/dependency"]),
    );
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/acme/features/published-feature:1\": {}\n  }\n}\n",
    )
    .expect("failed to write config");

    let lockfile =
        run_upgrade_lockfile(&["--workspace-folder".to_string(), root.display().to_string()])
            .expect("lockfile payload");

    assert_eq!(
        lockfile.features["ghcr.io/acme/features/published-feature:1"].version,
        "1.1.0"
    );
    assert_eq!(
        lockfile.features["ghcr.io/acme/features/published-feature:1"].resolved,
        format!("ghcr.io/acme/features/published-feature@sha256:{latest_digest}")
    );
    assert_eq!(
        lockfile.features["ghcr.io/acme/features/published-feature:1"].depends_on,
        Some(vec!["ghcr.io/acme/features/dependency".to_string()])
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ensure_native_lockfile_uses_shared_lockfile_format() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");

    ensure_native_lockfile_for_config(
        &["--workspace-folder".to_string(), root.display().to_string()],
        &config_file,
        &json!({
            "image": "debian:bookworm",
            "features": {
                "ghcr.io/devcontainers/features/github-cli": {}
            }
        }),
    )
    .expect("lockfile write");

    let lockfile = fs::read_to_string(root.join(".devcontainer-lock.json")).expect("lockfile");
    assert!(lockfile.ends_with('\n'));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ensure_native_lockfile_skips_generation_when_no_lockfile_is_set() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");

    ensure_native_lockfile_for_config(
        &[
            "--workspace-folder".to_string(),
            root.display().to_string(),
            "--no-lockfile".to_string(),
        ],
        &config_file,
        &json!({
            "image": "debian:bookworm",
            "features": {
                "ghcr.io/devcontainers/features/github-cli": {}
            }
        }),
    )
    .expect("lockfile skip");

    assert!(!root.join(".devcontainer-lock.json").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_lockfile_uses_shared_lockfile_format() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/github-cli\": {}\n  }\n}\n",
    )
    .expect("failed to write config");

    run_upgrade_lockfile(&["--workspace-folder".to_string(), root.display().to_string()])
        .expect("lockfile payload");

    let lockfile = fs::read_to_string(root.join(".devcontainer-lock.json")).expect("lockfile");
    assert!(lockfile.ends_with('\n'));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn ensure_native_lockfile_rejects_corrupt_existing_lockfile_when_generating() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");
    let lockfile_path = root.join(".devcontainer-lock.json");
    fs::write(&lockfile_path, "this is not json").expect("corrupt lockfile");

    let error = ensure_native_lockfile_for_config(
        &["--workspace-folder".to_string(), root.display().to_string()],
        &config_file,
        &json!({
            "image": "debian:bookworm",
            "features": {
                "ghcr.io/devcontainers/features/github-cli": {}
            }
        }),
    )
    .expect_err("corrupt lockfile error");

    assert!(error.contains("line 1 column"), "{error}");
    assert_eq!(
        fs::read_to_string(lockfile_path).expect("lockfile"),
        "this is not json"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ensure_native_lockfile_reports_missing_frozen_lockfile() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");

    let error = ensure_native_lockfile_for_config(
        &[
            "--workspace-folder".to_string(),
            root.display().to_string(),
            "--frozen-lockfile".to_string(),
        ],
        &config_file,
        &json!({
            "image": "debian:bookworm",
            "features": {
                "ghcr.io/devcontainers/features/github-cli": {}
            }
        }),
    )
    .expect_err("missing frozen lockfile error");

    assert_eq!(error, "Lockfile does not exist.");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ensure_native_lockfile_reports_outdated_frozen_lockfile() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");
    let configuration = json!({
        "image": "debian:bookworm",
        "features": {
            "ghcr.io/devcontainers/features/github-cli": {}
        }
    });
    fs::write(
        root.join(".devcontainer-lock.json"),
        "{\n  \"features\": {}\n}\n",
    )
    .expect("lockfile");

    let error = ensure_native_lockfile_for_config(
        &[
            "--workspace-folder".to_string(),
            root.display().to_string(),
            "--frozen-lockfile".to_string(),
        ],
        &config_file,
        &configuration,
    )
    .expect_err("outdated frozen lockfile error");

    assert!(error.contains("out of date"), "{error}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ensure_native_lockfile_accepts_semantically_identical_existing_json() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");
    ensure_native_lockfile_for_config(
        &["--workspace-folder".to_string(), root.display().to_string()],
        &config_file,
        &json!({
            "image": "debian:bookworm",
            "features": {
                "ghcr.io/devcontainers/features/github-cli": {}
            }
        }),
    )
    .expect("lockfile seed");
    let lockfile_path = root.join(".devcontainer-lock.json");
    let lockfile = fs::read_to_string(&lockfile_path).expect("lockfile");
    let reformatted = lockfile.trim_end_matches('\n').to_string();
    fs::write(&lockfile_path, reformatted).expect("lockfile rewrite");

    ensure_native_lockfile_for_config(
        &[
            "--workspace-folder".to_string(),
            root.display().to_string(),
            "--frozen-lockfile".to_string(),
        ],
        &config_file,
        &json!({
            "image": "debian:bookworm",
            "features": {
                "ghcr.io/devcontainers/features/github-cli": {}
            }
        }),
    )
    .expect("lockfile match");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn validate_native_lockfile_accepts_matching_frozen_lockfile() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");
    let configuration = json!({
        "image": "debian:bookworm",
        "features": {
            "ghcr.io/devcontainers/features/github-cli": {}
        }
    });
    ensure_native_lockfile_for_config(
        &["--workspace-folder".to_string(), root.display().to_string()],
        &config_file,
        &configuration,
    )
    .expect("lockfile seed");
    let frozen_args = vec![
        "--workspace-folder".to_string(),
        root.display().to_string(),
        "--frozen-lockfile".to_string(),
    ];
    let resolved_features =
        resolve_feature_support(&frozen_args, &root, &config_file, &configuration)
            .expect("feature support")
            .expect("resolved features");

    validate_native_lockfile(
        &frozen_args,
        &config_file,
        &configuration,
        &resolved_features,
    )
    .expect("matching frozen lockfile");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn validate_native_lockfile_reports_disabled_missing_and_outdated_states() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");
    let configuration = json!({
        "image": "debian:bookworm",
        "features": {
            "ghcr.io/devcontainers/features/github-cli": {}
        }
    });
    let resolved_features = resolve_feature_support(&[], &root, &config_file, &configuration)
        .expect("feature support")
        .expect("resolved features");

    validate_native_lockfile(
        &["--no-lockfile".to_string()],
        &config_file,
        &configuration,
        &resolved_features,
    )
    .expect("disabled validation");
    validate_native_lockfile(&[], &config_file, &configuration, &resolved_features)
        .expect("unfrozen validation");

    let frozen_args = vec!["--frozen-lockfile".to_string()];
    let missing_error = validate_native_lockfile(
        &frozen_args,
        &config_file,
        &configuration,
        &resolved_features,
    )
    .expect_err("missing lockfile");
    assert!(missing_error.contains("does not exist"), "{missing_error}");

    fs::write(
        root.join(".devcontainer-lock.json"),
        "{\n  \"features\": {}\n}\n",
    )
    .expect("stale lockfile");
    let outdated_error = validate_native_lockfile(
        &frozen_args,
        &config_file,
        &configuration,
        &resolved_features,
    )
    .expect_err("outdated lockfile");
    assert!(outdated_error.contains("out of date"), "{outdated_error}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn deprecated_experimental_frozen_lockfile_flag_is_still_reported() {
    warn_deprecated_lockfile_flags(&["--experimental-frozen-lockfile".to_string()]);
    warn_deprecated_lockfile_flags(&["--experimental-lockfile".to_string()]);
}

#[test]
fn validate_lockfile_options_rejects_mutually_exclusive_flags() {
    let error =
        validate_lockfile_options(&["--no-lockfile".to_string(), "--frozen-lockfile".to_string()])
            .expect_err("mutually exclusive lockfile flags");

    assert!(error.contains("mutually exclusive"), "{error}");
}

#[test]
fn lockfile_for_resolution_reports_non_file_lockfile_errors() {
    let root = unique_temp_dir();
    let config_dir = root.join(".devcontainer");
    fs::create_dir_all(&config_dir).expect("failed to create config dir");
    let config_file = config_dir.join("devcontainer.json");
    fs::write(&config_file, "{\n  \"image\": \"debian:bookworm\"\n}\n").expect("config");
    fs::create_dir(config_dir.join("devcontainer-lock.json")).expect("lockfile dir");

    let error = lockfile_for_resolution(&[], &config_file).expect_err("directory lockfile error");

    assert!(error.to_ascii_lowercase().contains("directory"), "{error}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn outdated_command_logs_absent_lockfile_at_debug_level() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": {}\n  }\n}\n",
    )
    .expect("failed to write config");

    let status = run_outdated(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
        "--log-level".to_string(),
        "debug".to_string(),
    ]);

    assert_eq!(status, ExitCode::SUCCESS);
    assert!(!root.join(".devcontainer-lock.json").exists());

    let payload =
        build_outdated_payload(&["--workspace-folder".to_string(), root.display().to_string()])
            .expect("outdated payload");
    assert_eq!(
        payload["features"]["ghcr.io/devcontainers/features/git:1.0"]["wanted"],
        "1.0.5"
    );

    let log = render_log(
        LogFormat::Text,
        CommandLogLevel::Debug,
        &format!(
            "No lockfile found at {}",
            root.join(".devcontainer-lock.json").display()
        ),
    );
    assert!(log.contains("No lockfile found"), "{log}");
    assert!(log.contains(".devcontainer-lock.json"), "{log}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn outdated_command_supports_text_output_json_logs_and_terminal_dimensions() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": {}\n  }\n}\n",
    )
    .expect("failed to write config");
    fs::write(
        root.join(".devcontainer-lock.json"),
        "{\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1.0\": {\n      \"version\": \"1.0.4\",\n      \"resolved\": \"ghcr.io/devcontainers/features/git@sha256:0bb490abcc0a3fb23937d29e2c18a225b51c5584edc0d9eb4131569a980f60b6\",\n      \"integrity\": \"sha256:0bb490abcc0a3fb23937d29e2c18a225b51c5584edc0d9eb4131569a980f60b6\"\n    }\n  }\n}\n",
    )
    .expect("failed to write lockfile");

    let status = run_outdated(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
        "--output-format".to_string(),
        "text".to_string(),
        "--log-format".to_string(),
        "json".to_string(),
        "--log-level".to_string(),
        "trace".to_string(),
        "--terminal-columns".to_string(),
        "120".to_string(),
        "--terminal-rows".to_string(),
        "40".to_string(),
    ]);

    assert_eq!(status, ExitCode::SUCCESS);

    let payload =
        build_outdated_payload(&["--workspace-folder".to_string(), root.display().to_string()])
            .expect("outdated payload");
    let text = render_outdated_text(&payload);
    assert!(text.contains("Feature"), "{text}");
    assert!(
        text.contains("ghcr.io/devcontainers/features/git"),
        "{text}"
    );
    assert!(text.contains("1.0.4"), "{text}");
    assert!(text.contains("1.0.5"), "{text}");
    assert!(text.contains("1.2.0"), "{text}");

    let json_output: serde_json::Value =
        serde_json::from_str(&payload.to_string()).expect("json output");
    assert_eq!(
        json_output["features"]["ghcr.io/devcontainers/features/git:1.0"]["latest"],
        "1.2.0"
    );

    let lockfile_log: serde_json::Value = serde_json::from_str(&render_log(
        LogFormat::Json,
        CommandLogLevel::Debug,
        &format!(
            "Loaded lockfile from {}",
            root.join(".devcontainer-lock.json").display()
        ),
    ))
    .expect("json lockfile log");
    assert_eq!(lockfile_log["type"], "text");
    assert_eq!(lockfile_log["level"], 2);
    assert!(lockfile_log["text"]
        .as_str()
        .is_some_and(|text| text.contains("Loaded lockfile from")));

    let terminal_log: serde_json::Value = serde_json::from_str(&render_log(
        LogFormat::Json,
        CommandLogLevel::Trace,
        "Using terminal dimensions: columns=120 rows=40",
    ))
    .expect("json terminal log");
    assert_eq!(terminal_log["level"], 1);
    assert_eq!(
        terminal_log["text"],
        "Using terminal dimensions: columns=120 rows=40"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_and_outdated_commands_reject_invalid_option_shapes() {
    assert_eq!(
        run_outdated(&["--definitely-unsupported".to_string()]),
        ExitCode::from(1)
    );
    assert_eq!(
        run_upgrade(&["--definitely-unsupported".to_string()]),
        ExitCode::from(1)
    );
    assert_eq!(
        run_upgrade(&[
            "--feature".to_string(),
            "ghcr.io/devcontainers/features/git".to_string(),
        ]),
        ExitCode::from(1)
    );
    assert_eq!(
        run_upgrade(&[
            "--feature".to_string(),
            "ghcr.io/devcontainers/features/git".to_string(),
            "--target-version".to_string(),
            "latest".to_string(),
        ]),
        ExitCode::from(1)
    );
}

#[test]
fn upgrade_lockfile_returns_empty_lockfile_without_configured_features() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\"\n}\n",
    )
    .expect("failed to write config");

    let lockfile =
        run_upgrade_lockfile(&["--workspace-folder".to_string(), root.display().to_string()])
            .expect("lockfile payload");

    assert!(lockfile.features.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_command_writes_lockfile_when_not_dry_run() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\"\n}\n",
    )
    .expect("failed to write config");

    let status = run_upgrade(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
        "--log-level".to_string(),
        "debug".to_string(),
    ]);

    assert_eq!(status, ExitCode::SUCCESS);
    let lockfile = fs::read_to_string(root.join(".devcontainer-lock.json")).expect("lockfile");
    assert!(lockfile.contains("\"features\": {}"), "{lockfile}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_lockfile_excludes_additional_only_features() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    fs::write(
        root.join(".devcontainer.json"),
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/github-cli\": {}\n  }\n}\n",
    )
    .expect("failed to write config");

    let lockfile = run_upgrade_lockfile(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
        "--additional-features".to_string(),
        "{\"ghcr.io/devcontainers/features/git\":{}}".to_string(),
    ])
    .expect("lockfile payload");

    assert!(lockfile
        .features
        .contains_key("ghcr.io/devcontainers/features/github-cli"));
    assert!(!lockfile
        .features
        .contains_key("ghcr.io/devcontainers/features/git"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_missing_feature_target_leaves_config_unchanged() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");
    let config = "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/github-cli\": {}\n  }\n}\n";
    fs::write(&config_file, config).expect("failed to write config");

    let status = run_upgrade(&[
        "--dry-run".to_string(),
        "--workspace-folder".to_string(),
        root.display().to_string(),
        "--feature".to_string(),
        "ghcr.io/devcontainers/features/git".to_string(),
        "--target-version".to_string(),
        "1".to_string(),
        "--log-level".to_string(),
        "trace".to_string(),
    ]);

    assert_eq!(status, ExitCode::SUCCESS);
    assert_eq!(fs::read_to_string(config_file).expect("config"), config);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn upgrade_feature_target_with_escaped_key_is_a_noop_update() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");
    let config = "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git\\u003a1\": {}\n  }\n}\n";
    fs::write(&config_file, config).expect("failed to write config");

    let status = run_upgrade(&[
        "--dry-run".to_string(),
        "--workspace-folder".to_string(),
        root.display().to_string(),
        "--feature".to_string(),
        "ghcr.io/devcontainers/features/git".to_string(),
        "--target-version".to_string(),
        "2".to_string(),
        "--log-level".to_string(),
        "trace".to_string(),
    ]);

    assert_eq!(status, ExitCode::SUCCESS);
    assert_eq!(fs::read_to_string(config_file).expect("config"), config);
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn upgrade_feature_update_reports_config_write_errors() {
    use std::os::unix::fs::PermissionsExt;

    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");
    let config_file = root.join(".devcontainer.json");
    fs::write(
        &config_file,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"ghcr.io/devcontainers/features/git:1\": {}\n  }\n}\n",
    )
    .expect("failed to write config");
    let mut permissions = fs::metadata(&config_file)
        .expect("config metadata")
        .permissions();
    permissions.set_mode(0o444);
    fs::set_permissions(&config_file, permissions).expect("readonly config");

    let status = run_upgrade(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
        "--feature".to_string(),
        "ghcr.io/devcontainers/features/git".to_string(),
        "--target-version".to_string(),
        "2".to_string(),
    ]);

    let mut permissions = fs::metadata(&config_file)
        .expect("config metadata")
        .permissions();
    permissions.set_mode(0o644);
    fs::set_permissions(&config_file, permissions).expect("writable config");
    assert_eq!(status, ExitCode::from(1));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn render_outdated_text_handles_payload_without_features() {
    assert_eq!(
        render_outdated_text(&json!({})),
        "Feature  Current  Wanted  Latest"
    );
}

#[test]
fn render_outdated_text_formats_feature_rows_and_missing_cells() {
    let text = render_outdated_text(&json!({
        "features": {
            "ghcr.io/devcontainers/features/git:1.0": {
                "current": "1.0.4",
                "wanted": null
            }
        }
    }));

    assert!(
        text.contains("ghcr.io/devcontainers/features/git"),
        "{text}"
    );
    assert!(text.contains("1.0.4"), "{text}");
    assert!(text.contains("-"), "{text}");
}

#[test]
fn read_configuration_native_support_rejects_positional_and_unknown_options() {
    assert!(!should_use_native_read_configuration(&[
        "positional".to_string()
    ]));
    assert!(!should_use_native_read_configuration(&[
        "--workspace-folder".to_string(),
        "/workspace".to_string(),
        "--unsupported".to_string(),
    ]));
}

#[test]
fn read_configuration_payload_returns_load_errors_without_container_fallback() {
    let root = unique_temp_dir();
    fs::create_dir_all(&root).expect("failed to create root");

    let error = build_read_configuration_payload(&[
        "--workspace-folder".to_string(),
        root.display().to_string(),
    ])
    .expect_err("missing config should be reported");

    assert!(
        error.contains("Unable to locate a dev container config"),
        "{error}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn merged_configuration_payload_accepts_non_object_configuration() {
    assert_eq!(
        merged_configuration_payload(&json!("not an object"), None, &[]),
        json!({})
    );
}

#[test]
fn configuration_facade_materializes_local_feature_installations() {
    let root = unique_temp_dir();
    let config_dir = root.join(".devcontainer");
    let feature_dir = config_dir.join("features").join("demo");
    fs::create_dir_all(&feature_dir).expect("feature dir");
    fs::write(
        feature_dir.join("devcontainer-feature.json"),
        "{\n  \"id\": \"demo\",\n  \"version\": \"1.0.0\",\n  \"name\": \"Demo\"\n}\n",
    )
    .expect("feature manifest");
    let config_file = config_dir.join("devcontainer.json");
    fs::write(
        &config_file,
        "{\n  \"image\": \"debian:bookworm\",\n  \"features\": {\n    \"./features/demo\": {}\n  }\n}\n",
    )
    .expect("config");
    let configuration = json!({
        "image": "debian:bookworm",
        "features": {
            "./features/demo": {}
        }
    });

    let resolved =
        resolve_feature_support_without_lockfile(&[], &root, &config_file, &configuration)
            .expect("resolve")
            .expect("resolved features");
    let installation = resolved.installations.first().expect("installation");
    assert_eq!(feature_installation_name(installation), "demo");

    let destination = root.join("materialized");
    materialize_feature_installation(installation, &destination).expect("materialize");
    assert!(destination.join("devcontainer-feature.json").is_file());
    assert!(destination.join("install.sh").is_file());
    let _ = fs::remove_dir_all(root);
}

fn ensure_native_lockfile_for_config(
    args: &[String],
    config_file: &Path,
    configuration: &serde_json::Value,
) -> Result<(), String> {
    let workspace_folder = config_file.parent().unwrap_or_else(|| Path::new("."));
    let resolved_features =
        resolve_feature_support(args, workspace_folder, config_file, configuration)?
            .expect("feature support");
    ensure_native_lockfile(args, config_file, configuration, &resolved_features)
}

fn write_workspace_layout_version(
    workspace_root: &Path,
    base: &str,
    version: &str,
    depends_on: Option<&[&str]>,
) -> String {
    let layout_dir = workspace_root
        .join(".devcontainer")
        .join("oci-layouts")
        .join(base);
    fs::create_dir_all(layout_dir.join("blobs").join("sha256")).expect("layout blobs");
    fs::write(
        layout_dir.join("oci-layout"),
        "{\n  \"imageLayoutVersion\": \"1.0.0\"\n}\n",
    )
    .expect("layout marker");

    let metadata = json!({
        "id": "published-feature",
        "version": version,
        "dependsOn": depends_on.map(<[_]>::to_vec),
    });
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "annotations": {
            "dev.containers.metadata": metadata.to_string(),
        }
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("manifest bytes");
    let digest = sha256_digest(&manifest_bytes);
    fs::write(
        layout_dir.join("blobs").join("sha256").join(&digest),
        &manifest_bytes,
    )
    .expect("manifest blob");

    let mut manifests = if layout_dir.join("index.json").is_file() {
        let index: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(layout_dir.join("index.json")).expect("index"),
        )
        .expect("index json");
        index["manifests"].as_array().cloned().unwrap_or_default()
    } else {
        Vec::new()
    };
    manifests.push(json!({
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "digest": format!("sha256:{digest}"),
        "size": manifest_bytes.len(),
        "annotations": {
            "org.opencontainers.image.ref.name": version,
        }
    }));
    fs::write(
        layout_dir.join("index.json"),
        serde_json::to_string_pretty(&json!({
            "schemaVersion": 2,
            "manifests": manifests,
        }))
        .expect("index payload"),
    )
    .expect("index write");

    digest
}

fn sha256_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

struct SingleResponseHttpServer {
    base_url: String,
}

impl SingleResponseHttpServer {
    fn new(body: &[u8]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("http listener");
        let address = listener.local_addr().expect("http listener address");
        let body = body.to_vec();
        thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request);
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(headers.as_bytes());
            let _ = stream.write_all(&body);
        });
        Self {
            base_url: format!("http://{address}"),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path)
    }
}
