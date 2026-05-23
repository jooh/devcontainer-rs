//! Manifest parsing and documentation helpers for collection commands.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config;

#[derive(Default)]
pub(crate) struct ManifestDocOptions {
    pub(crate) registry: Option<String>,
    pub(crate) namespace: Option<String>,
    pub(crate) github_owner: Option<String>,
    pub(crate) github_repo: Option<String>,
}

pub(crate) fn parse_manifest(root: &Path, manifest_name: &str) -> Result<Value, String> {
    let manifest_path = root.join(manifest_name);
    let raw = fs::read_to_string(&manifest_path).map_err(error_to_string)?;
    config::parse_jsonc_value(&raw)
}

pub(crate) fn generate_manifest_docs(
    root: &Path,
    manifest_name: &str,
    fallback_title: &str,
    options: &ManifestDocOptions,
) -> Result<PathBuf, String> {
    let manifest = parse_manifest(root, manifest_name)?;
    let readme_path = root.join("README.md");
    let name = manifest
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(fallback_title);
    let description = manifest
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("Generated documentation.");
    let mut contents = format!("# {name}\n\n{description}\n");
    if let (Some(registry), Some(namespace), Some(id)) = (
        options.registry.as_deref(),
        options.namespace.as_deref(),
        manifest.get("id").and_then(Value::as_str),
    ) {
        contents.push_str(&format!(
            "\n## OCI Reference\n\n`{registry}/{namespace}/{id}`\n"
        ));
    }
    if let (Some(owner), Some(repo)) = (
        options.github_owner.as_deref(),
        options.github_repo.as_deref(),
    ) {
        contents.push_str(&format!(
            "\n## Source Repository\n\nhttps://github.com/{owner}/{repo}\n"
        ));
    }
    fs::write(&readme_path, contents).map_err(error_to_string)?;
    Ok(readme_path)
}

fn error_to_string(error: impl ToString) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::test_support::unique_temp_dir;

    use super::{generate_manifest_docs, parse_manifest, ManifestDocOptions};

    fn assert_error_contains_any(error: &str, needles: &[&str]) {
        let normalized = error.to_lowercase();
        assert!(
            needles.iter().any(|needle| normalized.contains(needle)),
            "expected {error:?} to contain one of {needles:?}"
        );
    }

    #[test]
    fn parse_manifest_reports_missing_files_and_invalid_json() {
        let root = unique_temp_dir("manifest-parse-errors");
        fs::create_dir_all(&root).expect("root");

        let missing =
            parse_manifest(&root, "devcontainer-feature.json").expect_err("missing manifest");
        assert_error_contains_any(&missing, &["no such file", "not find"]);

        fs::write(root.join("devcontainer-feature.json"), "{").expect("invalid manifest");
        let invalid =
            parse_manifest(&root, "devcontainer-feature.json").expect_err("invalid manifest");
        assert_error_contains_any(&invalid, &["eof", "json"]);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn generate_manifest_docs_uses_fallbacks_and_optional_references() {
        let root = unique_temp_dir("manifest-docs");
        fs::create_dir_all(&root).expect("root");
        fs::write(
            root.join("devcontainer-feature.json"),
            r#"{
                "id": "demo"
            }"#,
        )
        .expect("manifest");

        let readme = generate_manifest_docs(
            &root,
            "devcontainer-feature.json",
            "Fallback Title",
            &ManifestDocOptions {
                registry: Some("ghcr.io".to_string()),
                namespace: Some("acme/features".to_string()),
                github_owner: Some("acme".to_string()),
                github_repo: Some("features".to_string()),
            },
        )
        .expect("docs");
        let contents = fs::read_to_string(readme).expect("readme");

        assert!(contents.contains("# Fallback Title"), "{contents}");
        assert!(contents.contains("Generated documentation."), "{contents}");
        assert!(
            contents.contains("`ghcr.io/acme/features/demo`"),
            "{contents}"
        );
        assert!(
            contents.contains("https://github.com/acme/features"),
            "{contents}"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn generate_manifest_docs_reports_readme_write_errors() {
        let root = unique_temp_dir("manifest-docs-write-error");
        fs::create_dir_all(root.join("README.md")).expect("readme collision");
        fs::write(
            root.join("devcontainer-feature.json"),
            r#"{
                "id": "demo",
                "name": "Demo",
                "description": "Demo docs."
            }"#,
        )
        .expect("manifest");

        let error = generate_manifest_docs(
            &root,
            "devcontainer-feature.json",
            "Fallback Title",
            &ManifestDocOptions::default(),
        )
        .expect_err("README.md directory should block write");

        assert_error_contains_any(&error, &["is a directory", "access is denied"]);
        let _ = fs::remove_dir_all(root);
    }
}
