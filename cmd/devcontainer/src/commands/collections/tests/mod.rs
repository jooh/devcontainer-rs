//! Unit test entrypoints for collection command modules.

use std::fs;
use std::process::ExitCode;

mod feature_tests;
mod features;
mod publish;
mod support;
mod templates;

#[test]
fn collection_entrypoints_report_missing_and_unknown_subcommands() {
    assert_eq!(super::run_features(&[]), ExitCode::from(1));
    assert_eq!(
        super::run_features(&["unknown".to_string()]),
        ExitCode::from(1)
    );
    assert_eq!(
        super::run_features(&["info".to_string(), "manifest".to_string()]),
        ExitCode::from(1)
    );
    assert_eq!(super::run_templates(&[]), ExitCode::from(1));
    assert_eq!(
        super::run_templates(&["unknown".to_string()]),
        ExitCode::from(1)
    );
    assert_eq!(
        super::run_templates(&["metadata".to_string()]),
        ExitCode::from(1)
    );
}

#[test]
fn collection_entrypoints_run_package_publish_and_docs_paths() {
    let root = support::unique_temp_dir();
    let feature_output = support::unique_temp_dir();
    fs::create_dir_all(&root).expect("feature root");
    fs::write(
        root.join("devcontainer-feature.json"),
        "{\n  \"id\": \"entrypoint-feature\",\n  \"name\": \"Entrypoint Feature\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("feature manifest");

    assert_eq!(
        super::run_features(&["package".to_string(), root.display().to_string()]),
        ExitCode::SUCCESS
    );
    assert_eq!(
        super::run_features(&[
            "generate-docs".to_string(),
            root.display().to_string(),
            "--registry".to_string(),
            "ghcr.io".to_string(),
            "--namespace".to_string(),
            "acme/features".to_string(),
            "--github-owner".to_string(),
            "acme".to_string(),
            "--github-repo".to_string(),
            "features".to_string(),
        ]),
        ExitCode::SUCCESS
    );
    assert!(root.join("README.md").is_file());
    assert_eq!(
        super::run_features(&[
            "publish".to_string(),
            root.display().to_string(),
            "--output-dir".to_string(),
            feature_output.display().to_string(),
        ]),
        ExitCode::SUCCESS
    );
    assert!(feature_output.join("oci-layout").is_file());

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(feature_output);
}

#[test]
fn template_entrypoints_run_metadata_publish_and_docs_paths() {
    let root = support::unique_temp_dir();
    let output = support::unique_temp_dir();
    fs::create_dir_all(&root).expect("template root");
    fs::write(
        root.join("devcontainer-template.json"),
        "{\n  \"id\": \"entrypoint-template\",\n  \"name\": \"Entrypoint Template\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .expect("template manifest");

    assert_eq!(
        super::run_templates(&["metadata".to_string(), root.display().to_string()]),
        ExitCode::SUCCESS
    );
    assert_eq!(
        super::run_templates(&[
            "generate-docs".to_string(),
            root.display().to_string(),
            "--github-owner".to_string(),
            "acme".to_string(),
            "--github-repo".to_string(),
            "templates".to_string(),
        ]),
        ExitCode::SUCCESS
    );
    assert!(root.join("README.md").is_file());
    assert_eq!(
        super::run_templates(&[
            "publish".to_string(),
            root.display().to_string(),
            "--output-dir".to_string(),
            output.display().to_string(),
        ]),
        ExitCode::SUCCESS
    );
    assert!(output.join("index.json").is_file());

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(output);
}
