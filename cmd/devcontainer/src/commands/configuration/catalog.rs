//! Version catalog helpers for configuration upgrade and outdated commands.

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::{CatalogEntry, FeatureReference, Lockfile, ParsedVersion};
use crate::commands::collections::oci;

pub(super) fn build_feature_version_info(
    feature: &FeatureReference,
    lockfile: Option<&Lockfile>,
    workspace_folder: Option<&Path>,
) -> Result<Option<Value>, String> {
    let current = lockfile
        .and_then(|value| value.features.get(&feature.original))
        .map(|entry| entry.version.clone());

    if oci::is_registry_qualified_reference(&feature.base) {
        let wanted_artifact = oci::resolve_feature_artifact(&feature.original, workspace_folder)?;
        let wanted = wanted_artifact
            .metadata
            .get("version")
            .and_then(Value::as_str)
            .or(wanted_artifact.tag.as_deref())
            .unwrap_or("latest")
            .to_string();
        let latest = latest_oci_version(&feature.base, workspace_folder)?;
        return Ok(Some(version_info_json(
            current.or_else(|| Some(wanted.clone())),
            Some(wanted.clone()),
            latest.clone(),
            major_string(&wanted),
            latest.as_deref().and_then(major_string),
        )));
    }

    if feature.digest.is_some() {
        let wanted = current.clone().or_else(|| {
            exact_catalog_entry(&feature.original, workspace_folder).map(|entry| entry.version)
        });
        let latest = latest_version(&feature.base, workspace_folder);
        return Ok(Some(version_info_json(
            current.or_else(|| wanted.clone()),
            wanted.clone(),
            latest.clone(),
            wanted.as_deref().and_then(major_string),
            latest.as_deref().and_then(major_string),
        )));
    }

    let latest = latest_version(&feature.base, workspace_folder);
    let wanted = resolve_wanted_version(feature, lockfile, workspace_folder);
    if latest.is_none() && wanted.is_none() && current.is_none() {
        return Ok(Some(version_info_json(None, None, None, None, None)));
    }

    Ok(Some(version_info_json(
        current.or_else(|| wanted.clone()),
        wanted.clone(),
        latest.clone(),
        wanted.as_deref().and_then(major_string),
        latest.as_deref().and_then(major_string),
    )))
}

fn latest_oci_version(
    base: &str,
    workspace_folder: Option<&Path>,
) -> Result<Option<String>, String> {
    let mut tags = oci::list_feature_tags(base, workspace_folder)?
        .into_iter()
        .filter(|tag| is_exact_semver(tag))
        .collect::<Vec<_>>();
    tags.sort_by(|left, right| compare_versions_desc(left, right));
    if let Some(tag) = tags.into_iter().next() {
        return Ok(Some(tag));
    }
    oci::resolve_feature_artifact(base, workspace_folder).map(|artifact| {
        artifact
            .metadata
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

pub(super) fn resolve_wanted_version(
    feature: &FeatureReference,
    lockfile: Option<&Lockfile>,
    workspace_folder: Option<&Path>,
) -> Option<String> {
    if let Some(entry) = lockfile.and_then(|value| value.features.get(&feature.original)) {
        if feature.tag.is_none() || feature.digest.is_some() {
            return Some(entry.version.clone());
        }
    }

    let tag = feature.tag.as_deref()?;
    if tag == "latest" {
        return latest_version(&feature.base, workspace_folder);
    }

    let candidates = catalog_entries(&feature.base, workspace_folder)?;
    if tag.matches('.').count() == 2 {
        return candidates
            .iter()
            .find(|entry| entry.version == tag)
            .map(|entry| entry.version.to_string());
    }

    let selector = parse_selector(tag)?;
    candidates
        .iter()
        .find(|entry| selector.matches(&entry.version))
        .map(|entry| entry.version.to_string())
}

pub(super) fn exact_catalog_entry(
    feature_id: &str,
    workspace_folder: Option<&Path>,
) -> Option<CatalogEntry> {
    if let Some(entry) = local_oci_layout_exact_entry(feature_id, workspace_folder) {
        return Some(entry);
    }

    if feature_id
        == "ghcr.io/devcontainers/features/git-lfs@sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c"
    {
        return Some(CatalogEntry {
            version: "1.0.6".to_string(),
            resolved: "ghcr.io/devcontainers/features/git-lfs@sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c".to_string(),
            integrity: "sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c".to_string(),
            depends_on: None,
        });
    }

    fixture_catalog()
        .into_iter()
        .find(|(catalog_feature_id, _)| catalog_feature_id == feature_id)
        .map(|(_, entry)| entry)
}

pub(crate) fn catalog_entries(
    base: &str,
    workspace_folder: Option<&Path>,
) -> Option<Vec<CatalogEntry>> {
    let mut entries = local_oci_layout_entries(base, workspace_folder);
    entries.extend(
        manual_catalog_entries()
            .into_iter()
            .filter(|(catalog_base, _)| catalog_base == base)
            .map(|(_, entry)| entry),
    );
    entries.extend(
        fixture_catalog()
            .into_iter()
            .filter(|(feature_id, _)| {
                super::upgrade::feature_id_without_version(feature_id) == base
            })
            .map(|(_, entry)| entry),
    );
    entries.sort_by(|left, right| compare_versions_desc(&left.version, &right.version));
    let mut seen_versions = std::collections::BTreeSet::new();
    entries.retain(|entry| seen_versions.insert(entry.version.clone()));
    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

pub(super) fn latest_version(base: &str, workspace_folder: Option<&Path>) -> Option<String> {
    catalog_entries(base, workspace_folder)
        .and_then(|entries| entries.first().cloned())
        .map(|entry| entry.version)
}

fn local_oci_layout_entries(base: &str, workspace_folder: Option<&Path>) -> Vec<CatalogEntry> {
    let Some(layout_dir) = workspace_oci_layout_dir(base, workspace_folder) else {
        return Vec::new();
    };

    local_oci_index_manifests(&layout_dir)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| {
            let tag = entry["annotations"]["org.opencontainers.image.ref.name"].as_str()?;
            if !is_exact_semver(tag) {
                return None;
            }

            let digest = entry["digest"].as_str()?.strip_prefix("sha256:")?;
            catalog_entry_from_layout_manifest(base, &layout_dir, digest, Some(tag))
        })
        .collect()
}

fn local_oci_layout_exact_entry(
    feature_id: &str,
    workspace_folder: Option<&Path>,
) -> Option<CatalogEntry> {
    let base = super::upgrade::feature_id_without_version(feature_id);
    let digest = feature_id.rsplit_once("@sha256:")?.1;
    let layout_dir = workspace_oci_layout_dir(&base, workspace_folder)?;
    catalog_entry_from_layout_manifest(&base, &layout_dir, digest, None)
}

fn workspace_oci_layout_dir(base: &str, workspace_folder: Option<&Path>) -> Option<PathBuf> {
    if !base.starts_with("ghcr.io/") {
        return None;
    }

    let layout_dir = workspace_folder?
        .join(".devcontainer")
        .join("oci-layouts")
        .join(base);
    if layout_dir.join("oci-layout").is_file() {
        Some(layout_dir)
    } else {
        None
    }
}

fn local_oci_index_manifests(layout_dir: &Path) -> Result<Vec<Value>, String> {
    let index: Value = serde_json::from_str(
        &fs::read_to_string(layout_dir.join("index.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok(index["manifests"].as_array().cloned().unwrap_or_default())
}

fn catalog_entry_from_layout_manifest(
    base: &str,
    layout_dir: &Path,
    digest: &str,
    tag: Option<&str>,
) -> Option<CatalogEntry> {
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(layout_dir.join("blobs").join("sha256").join(digest)).ok()?,
    )
    .ok()?;
    let metadata = manifest["annotations"]["dev.containers.metadata"]
        .as_str()
        .and_then(|value| serde_json::from_str::<Value>(value).ok())?;
    let version = metadata
        .get("version")
        .and_then(Value::as_str)
        .or(tag)?
        .to_string();
    Some(CatalogEntry {
        version,
        resolved: format!("{base}@sha256:{digest}"),
        integrity: format!("sha256:{digest}"),
        depends_on: metadata["dependsOn"].as_array().map(|entries| {
            entries
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        }),
    })
}

fn is_exact_semver(input: &str) -> bool {
    matches!(parse_selector(input), Some(VersionSelector::Exact(_)))
}

fn version_info_json(
    current: Option<String>,
    wanted: Option<String>,
    latest: Option<String>,
    wanted_major: Option<String>,
    latest_major: Option<String>,
) -> Value {
    let mut entries = Map::new();
    if let Some(value) = current {
        entries.insert("current".to_string(), Value::String(value));
    }
    if let Some(value) = wanted {
        entries.insert("wanted".to_string(), Value::String(value));
    }
    if let Some(value) = latest {
        entries.insert("latest".to_string(), Value::String(value));
    }
    if let Some(value) = wanted_major {
        entries.insert("wantedMajor".to_string(), Value::String(value));
    }
    if let Some(value) = latest_major {
        entries.insert("latestMajor".to_string(), Value::String(value));
    }
    Value::Object(entries)
}

fn manual_catalog_entries() -> Vec<(String, CatalogEntry)> {
    vec![
        (
            "ghcr.io/devcontainers/features/git".to_string(),
            CatalogEntry {
                version: "1.2.0".to_string(),
                resolved: "ghcr.io/devcontainers/features/git@sha256:1111111111111111111111111111111111111111111111111111111111111111".to_string(),
                integrity: "sha256:1111111111111111111111111111111111111111111111111111111111111111".to_string(),
                depends_on: None,
            },
        ),
        (
            "ghcr.io/devcontainers/features/git".to_string(),
            CatalogEntry {
                version: "1.1.5".to_string(),
                resolved: "ghcr.io/devcontainers/features/git@sha256:2ab83ca71d55d5c00a1255b07f3a83a53cd2de77ce8b9637abad38095d672a5b".to_string(),
                integrity: "sha256:2ab83ca71d55d5c00a1255b07f3a83a53cd2de77ce8b9637abad38095d672a5b".to_string(),
                depends_on: None,
            },
        ),
        (
            "ghcr.io/devcontainers/features/git".to_string(),
            CatalogEntry {
                version: "1.0.5".to_string(),
                resolved: "ghcr.io/devcontainers/features/git@sha256:2222222222222222222222222222222222222222222222222222222222222222".to_string(),
                integrity: "sha256:2222222222222222222222222222222222222222222222222222222222222222".to_string(),
                depends_on: None,
            },
        ),
        (
            "ghcr.io/devcontainers/features/git".to_string(),
            CatalogEntry {
                version: "1.0.4".to_string(),
                resolved: "ghcr.io/devcontainers/features/git@sha256:0bb490abcc0a3fb23937d29e2c18a225b51c5584edc0d9eb4131569a980f60b6".to_string(),
                integrity: "sha256:0bb490abcc0a3fb23937d29e2c18a225b51c5584edc0d9eb4131569a980f60b6".to_string(),
                depends_on: None,
            },
        ),
        (
            "ghcr.io/devcontainers/features/github-cli".to_string(),
            CatalogEntry {
                version: "1.0.9".to_string(),
                resolved: "ghcr.io/devcontainers/features/github-cli@sha256:9024deeca80347dea7603a3bb5b4951988f0bf5894ba036a6ee3f29c025692c6".to_string(),
                integrity: "sha256:9024deeca80347dea7603a3bb5b4951988f0bf5894ba036a6ee3f29c025692c6".to_string(),
                depends_on: None,
            },
        ),
        (
            "ghcr.io/devcontainers/features/azure-cli".to_string(),
            CatalogEntry {
                version: "1.2.1".to_string(),
                resolved: "ghcr.io/devcontainers/features/azure-cli@sha256:a00aa292592a8df58a940d6f6dfcf2bfd3efab145f62a17ccb12656528793134".to_string(),
                integrity: "sha256:a00aa292592a8df58a940d6f6dfcf2bfd3efab145f62a17ccb12656528793134".to_string(),
                depends_on: None,
            },
        ),
        (
            "ghcr.io/codspace/versioning/foo".to_string(),
            CatalogEntry {
                version: "2.11.1".to_string(),
                resolved: "ghcr.io/codspace/versioning/foo@sha256:3333333333333333333333333333333333333333333333333333333333333333".to_string(),
                integrity: "sha256:3333333333333333333333333333333333333333333333333333333333333333".to_string(),
                depends_on: None,
            },
        ),
        (
            "ghcr.io/codspace/versioning/foo".to_string(),
            CatalogEntry {
                version: "0.3.1".to_string(),
                resolved: "ghcr.io/codspace/versioning/foo@sha256:4444444444444444444444444444444444444444444444444444444444444444".to_string(),
                integrity: "sha256:4444444444444444444444444444444444444444444444444444444444444444".to_string(),
                depends_on: None,
            },
        ),
        (
            "ghcr.io/codspace/versioning/bar".to_string(),
            CatalogEntry {
                version: "1.0.0".to_string(),
                resolved: "ghcr.io/codspace/versioning/bar@sha256:5555555555555555555555555555555555555555555555555555555555555555".to_string(),
                integrity: "sha256:5555555555555555555555555555555555555555555555555555555555555555".to_string(),
                depends_on: None,
            },
        ),
    ]
}

fn fixture_catalog() -> Vec<(String, CatalogEntry)> {
    vec![
        (
            "ghcr.io/devcontainers/features/azure-cli:1.2.1".to_string(),
            CatalogEntry {
                version: "1.2.1".to_string(),
                resolved: "ghcr.io/devcontainers/features/azure-cli@sha256:a00aa292592a8df58a940d6f6dfcf2bfd3efab145f62a17ccb12656528793134".to_string(),
                integrity: "sha256:a00aa292592a8df58a940d6f6dfcf2bfd3efab145f62a17ccb12656528793134".to_string(),
                depends_on: None,
            },
        ),
        (
            "ghcr.io/devcontainers/features/git-lfs@sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c".to_string(),
            CatalogEntry {
                version: "1.0.6".to_string(),
                resolved: "ghcr.io/devcontainers/features/git-lfs@sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c".to_string(),
                integrity: "sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c".to_string(),
                depends_on: None,
            },
        ),
        (
            "ghcr.io/devcontainers/features/git:1.1.5".to_string(),
            CatalogEntry {
                version: "1.1.5".to_string(),
                resolved: "ghcr.io/devcontainers/features/git@sha256:2ab83ca71d55d5c00a1255b07f3a83a53cd2de77ce8b9637abad38095d672a5b".to_string(),
                integrity: "sha256:2ab83ca71d55d5c00a1255b07f3a83a53cd2de77ce8b9637abad38095d672a5b".to_string(),
                depends_on: None,
            },
        ),
        (
            "ghcr.io/devcontainers/features/github-cli:1.0.9".to_string(),
            CatalogEntry {
                version: "1.0.9".to_string(),
                resolved: "ghcr.io/devcontainers/features/github-cli@sha256:9024deeca80347dea7603a3bb5b4951988f0bf5894ba036a6ee3f29c025692c6".to_string(),
                integrity: "sha256:9024deeca80347dea7603a3bb5b4951988f0bf5894ba036a6ee3f29c025692c6".to_string(),
                depends_on: None,
            },
        ),
        (
            "ghcr.io/codspace/dependson/A:2".to_string(),
            CatalogEntry {
                version: "2.0.1".to_string(),
                resolved: "ghcr.io/codspace/dependson/a@sha256:932027ef71da186210e6ceb3294c3459caaf6b548d2b547d5d26be3fc4b2264a".to_string(),
                integrity: "sha256:932027ef71da186210e6ceb3294c3459caaf6b548d2b547d5d26be3fc4b2264a".to_string(),
                depends_on: Some(vec!["ghcr.io/codspace/dependson/E".to_string()]),
            },
        ),
        (
            "ghcr.io/codspace/dependson/E".to_string(),
            CatalogEntry {
                version: "2.0.0".to_string(),
                resolved: "ghcr.io/codspace/dependson/e@sha256:9f36f159c70f8bebff57f341904b030733adb17ef12a5d58d4b3d89b2a6c7d5a".to_string(),
                integrity: "sha256:9f36f159c70f8bebff57f341904b030733adb17ef12a5d58d4b3d89b2a6c7d5a".to_string(),
                depends_on: None,
            },
        ),
        (
            "ghcr.io/codspace/dependson/E:1".to_string(),
            CatalogEntry {
                version: "1.0.0".to_string(),
                resolved: "ghcr.io/codspace/dependson/e@sha256:90b84127edab28ecb169cd6c6f2101ce0ea1d77589cee01951fec7f879f3a11c".to_string(),
                integrity: "sha256:90b84127edab28ecb169cd6c6f2101ce0ea1d77589cee01951fec7f879f3a11c".to_string(),
                depends_on: None,
            },
        ),
        (
            "https://github.com/codspace/tgz-features-with-dependson/releases/download/0.0.2/devcontainer-feature-A.tgz".to_string(),
            CatalogEntry {
                version: "2.0.1".to_string(),
                resolved: "https://github.com/codspace/tgz-features-with-dependson/releases/download/0.0.2/devcontainer-feature-A.tgz".to_string(),
                integrity: "sha256:f2dd5be682cceedb5497f9a734b5d5e7834424ade75b8cc700927242585ec671".to_string(),
                depends_on: Some(vec!["ghcr.io/codspace/dependson/E".to_string()]),
            },
        ),
        (
            "https://github.com/codspace/tgz-features-with-dependson/releases/download/0.0.2/devcontainer-feature-B.tgz".to_string(),
            CatalogEntry {
                version: "0.0.2".to_string(),
                resolved: "https://github.com/codspace/tgz-features-with-dependson/releases/download/0.0.2/devcontainer-feature-B.tgz".to_string(),
                integrity: "sha256:d130123ba54335a026ab6cd51c8bcbd52d58a0aeaacd8a593512ba61c5117ea0".to_string(),
                depends_on: Some(vec![
                    "ghcr.io/codspace/dependson/C".to_string(),
                    "ghcr.io/codspace/dependson/D".to_string(),
                ]),
            },
        ),
    ]
}

fn compare_versions_desc(left: &str, right: &str) -> Ordering {
    match (parse_version(left), parse_version(right)) {
        (Some(left_version), Some(right_version)) => right_version.cmp(&left_version),
        _ => right.cmp(left),
    }
}

fn parse_selector(input: &str) -> Option<VersionSelector> {
    let parts = input
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()?;
    match parts.as_slice() {
        [major] => Some(VersionSelector::Major(*major)),
        [major, minor] => Some(VersionSelector::MajorMinor(*major, *minor)),
        [major, minor, patch] => Some(VersionSelector::Exact(ParsedVersion {
            major: *major,
            minor: *minor,
            patch: *patch,
        })),
        _ => None,
    }
}

fn parse_version(input: &str) -> Option<ParsedVersion> {
    let selector = parse_selector(input)?;
    match selector {
        VersionSelector::Major(major) => Some(ParsedVersion {
            major,
            minor: 0,
            patch: 0,
        }),
        VersionSelector::MajorMinor(major, minor) => Some(ParsedVersion {
            major,
            minor,
            patch: 0,
        }),
        VersionSelector::Exact(version) => Some(version),
    }
}

fn major_string(input: &str) -> Option<String> {
    parse_version(input).map(|version| version.major.to_string())
}

enum VersionSelector {
    Major(u64),
    MajorMinor(u64, u64),
    Exact(ParsedVersion),
}

impl VersionSelector {
    fn matches(&self, version: &str) -> bool {
        let Some(parsed) = parse_version(version) else {
            return false;
        };
        match self {
            VersionSelector::Major(major) => parsed.major == *major,
            VersionSelector::MajorMinor(major, minor) => {
                parsed.major == *major && parsed.minor == *minor
            }
            VersionSelector::Exact(expected) => parsed == *expected,
        }
    }
}

impl Ord for ParsedVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch))
    }
}

impl PartialOrd for ParsedVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::collections::BTreeMap;
    use std::fs;

    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::super::{FeatureReference, Lockfile, LockfileEntry, ParsedVersion};
    use super::{
        build_feature_version_info, catalog_entries, compare_versions_desc, exact_catalog_entry,
        latest_oci_version, major_string, parse_selector, parse_version, resolve_wanted_version,
    };

    fn feature_ref(
        original: &str,
        base: &str,
        tag: Option<&str>,
        digest: Option<&str>,
    ) -> FeatureReference {
        FeatureReference {
            original: original.to_string(),
            base: base.to_string(),
            tag: tag.map(str::to_string),
            digest: digest.map(str::to_string),
        }
    }

    fn lockfile_with(feature_id: &str, version: &str) -> Lockfile {
        Lockfile {
            features: BTreeMap::from([(
                feature_id.to_string(),
                LockfileEntry {
                    version: version.to_string(),
                    resolved: format!("{feature_id}@sha256:locked"),
                    integrity: "sha256:locked".to_string(),
                    depends_on: None,
                },
            )]),
        }
    }

    fn write_layout_version(
        workspace_root: &std::path::Path,
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

    fn replace_layout_tags(workspace_root: &std::path::Path, base: &str, tags: &[(&str, &str)]) {
        let layout_dir = workspace_root
            .join(".devcontainer")
            .join("oci-layouts")
            .join(base);
        let manifests = tags
            .iter()
            .map(|(tag, digest)| {
                let size = fs::metadata(layout_dir.join("blobs").join("sha256").join(digest))
                    .expect("manifest blob metadata")
                    .len();
                json!({
                    "mediaType": "application/vnd.oci.image.manifest.v1+json",
                    "digest": format!("sha256:{digest}"),
                    "size": size,
                    "annotations": {
                        "org.opencontainers.image.ref.name": tag,
                    }
                })
            })
            .collect::<Vec<_>>();
        fs::write(
            layout_dir.join("index.json"),
            serde_json::to_string_pretty(&json!({
                "schemaVersion": 2,
                "manifests": manifests,
            }))
            .expect("index payload"),
        )
        .expect("index write");
    }

    fn sha256_digest(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }

    #[test]
    fn fixture_catalog_keeps_dependson_edges() {
        let entry = exact_catalog_entry("ghcr.io/codspace/dependson/A:2", None)
            .expect("dependson fixture entry");

        assert_eq!(entry.version, "2.0.1");
        assert_eq!(
            entry.depends_on,
            Some(vec!["ghcr.io/codspace/dependson/E".to_string()])
        );
    }

    #[test]
    fn fixture_catalog_exposes_upgrade_versions() {
        let entries = catalog_entries("ghcr.io/devcontainers/features/git", None)
            .expect("git catalog entries");

        assert!(entries.iter().any(|entry| entry.version == "1.1.5"));
    }

    #[test]
    fn workspace_oci_layout_entries_override_static_catalogs() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-catalog-test");
        write_layout_version(
            &workspace,
            "ghcr.io/devcontainers/features/git",
            "9.9.9",
            None,
        );

        let entries = catalog_entries(
            "ghcr.io/devcontainers/features/git",
            Some(workspace.as_path()),
        )
        .expect("git catalog entries");

        assert_eq!(entries.first().expect("first entry").version, "9.9.9");
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn workspace_oci_layout_entries_ignore_moving_tags_and_append_versions() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-catalog-test");
        let base = "ghcr.io/acme/features/published-feature";
        let first_digest = write_layout_version(&workspace, base, "1.0.0", None);
        let second_digest = write_layout_version(&workspace, base, "1.1.0", None);
        replace_layout_tags(
            &workspace,
            base,
            &[
                ("latest", &second_digest),
                ("1.0", &first_digest),
                ("1.0.0", &first_digest),
                ("1.1.0", &second_digest),
            ],
        );

        let entries = catalog_entries(base, Some(workspace.as_path())).expect("layout entries");

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.version.as_str())
                .collect::<Vec<_>>(),
            vec!["1.1.0", "1.0.0"]
        );
        assert!(super::workspace_oci_layout_dir(
            "ghcr.io/acme/features/missing-layout",
            Some(workspace.as_path())
        )
        .is_none());
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn workspace_oci_layout_entries_ignore_unreadable_or_incomplete_layouts() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-catalog-test");
        let base = "ghcr.io/acme/features/incomplete-layout";
        let layout_dir = workspace
            .join(".devcontainer")
            .join("oci-layouts")
            .join(base);
        fs::create_dir_all(&layout_dir).expect("layout dir");
        fs::write(layout_dir.join("oci-layout"), "{}").expect("layout marker");

        assert!(catalog_entries(base, Some(workspace.as_path())).is_none());
        assert!(catalog_entries("example.com/not-ghcr", Some(workspace.as_path())).is_none());
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn latest_oci_version_ignores_moving_semantic_tags() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-catalog-test");
        let base = "ghcr.io/acme/features/published-feature";
        let digest = write_layout_version(&workspace, base, "2.0.0", None);
        replace_layout_tags(
            &workspace,
            base,
            &[
                ("2", &digest),
                ("2.0", &digest),
                ("2.0.0", &digest),
                ("latest", &digest),
            ],
        );

        let latest = latest_oci_version(base, Some(workspace.as_path())).expect("latest version");

        assert_eq!(latest.as_deref(), Some("2.0.0"));
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn workspace_oci_layout_supports_exact_digest_lookup() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-catalog-test");
        let digest = write_layout_version(
            &workspace,
            "ghcr.io/acme/features/published-feature",
            "1.0.1",
            Some(&["ghcr.io/acme/features/dependency"]),
        );

        let entry = exact_catalog_entry(
            &format!("ghcr.io/acme/features/published-feature@sha256:{digest}"),
            Some(workspace.as_path()),
        )
        .expect("layout entry");

        assert_eq!(entry.version, "1.0.1");
        assert_eq!(
            entry.depends_on,
            Some(vec!["ghcr.io/acme/features/dependency".to_string()])
        );
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn exact_catalog_entry_exposes_static_digest_pinned_entries() {
        let entry = exact_catalog_entry(
            "ghcr.io/devcontainers/features/git-lfs@sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c",
            None,
        )
        .expect("git-lfs entry");

        assert_eq!(entry.version, "1.0.6");
        assert_eq!(
            entry.integrity,
            "sha256:24d5802c837b2519b666a8403a9514c7296d769c9607048e9f1e040e7d7e331c"
        );
    }

    #[test]
    fn resolve_wanted_version_prefers_lockfile_latest_and_selectors() {
        let locked = lockfile_with("ghcr.io/devcontainers/features/git", "1.0.4");
        let locked_exact = lockfile_with("ghcr.io/devcontainers/features/git:1.1.5", "9.9.9");
        let untagged = feature_ref(
            "ghcr.io/devcontainers/features/git",
            "ghcr.io/devcontainers/features/git",
            None,
            None,
        );
        let latest = feature_ref(
            "ghcr.io/devcontainers/features/git:latest",
            "ghcr.io/devcontainers/features/git",
            Some("latest"),
            None,
        );
        let exact = feature_ref(
            "ghcr.io/devcontainers/features/git:1.1.5",
            "ghcr.io/devcontainers/features/git",
            Some("1.1.5"),
            None,
        );
        let major_minor = feature_ref(
            "ghcr.io/devcontainers/features/git:1.0",
            "ghcr.io/devcontainers/features/git",
            Some("1.0"),
            None,
        );
        let invalid = feature_ref(
            "ghcr.io/devcontainers/features/git:not-a-version",
            "ghcr.io/devcontainers/features/git",
            Some("not-a-version"),
            None,
        );

        assert_eq!(
            resolve_wanted_version(&untagged, Some(&locked), None).as_deref(),
            Some("1.0.4")
        );
        assert_eq!(
            resolve_wanted_version(&exact, Some(&locked_exact), None).as_deref(),
            Some("1.1.5")
        );
        assert_eq!(
            resolve_wanted_version(&latest, None, None).as_deref(),
            Some("1.2.0")
        );
        assert_eq!(
            resolve_wanted_version(&exact, None, None).as_deref(),
            Some("1.1.5")
        );
        assert_eq!(
            resolve_wanted_version(&major_minor, None, None).as_deref(),
            Some("1.0.5")
        );
        assert_eq!(resolve_wanted_version(&invalid, None, None), None);
    }

    #[test]
    fn build_feature_version_info_handles_oci_digest_and_unknown_features() {
        let oci = feature_ref(
            "ghcr.io/devcontainers/features/git:1",
            "ghcr.io/devcontainers/features/git",
            Some("1"),
            None,
        );
        let catalog = feature_ref(
            "ghcr.io/devcontainers/features/git:1.0",
            "ghcr.io/devcontainers/features/git",
            Some("1.0"),
            None,
        );
        let digest = feature_ref(
            "https://example.com/feature.tgz@sha256:abc",
            "https://example.com/feature.tgz",
            None,
            Some("sha256:abc"),
        );
        let unknown = feature_ref("example-feature", "example-feature", None, None);
        let locked_unknown = lockfile_with("example-feature", "9.9.9");

        let oci_info = build_feature_version_info(&oci, None, None)
            .expect("oci info")
            .expect("oci payload");
        let catalog_info = build_feature_version_info(&catalog, None, None)
            .expect("catalog info")
            .expect("catalog payload");
        let digest_info = build_feature_version_info(&digest, None, None)
            .expect("digest info")
            .expect("digest payload");
        let unknown_info = build_feature_version_info(&unknown, None, None)
            .expect("unknown info")
            .expect("unknown payload");
        let locked_unknown_info = build_feature_version_info(&unknown, Some(&locked_unknown), None)
            .expect("locked unknown info")
            .expect("locked unknown payload");

        assert!(oci_info.get("wanted").is_some());
        assert!(oci_info.get("latest").is_some());
        assert_eq!(catalog_info["wanted"], "1.0.5");
        assert_eq!(catalog_info["latest"], "1.2.0");
        assert_eq!(catalog_info["wantedMajor"], "1");
        assert_eq!(catalog_info["latestMajor"], "1");
        assert_eq!(digest_info, json!({}));
        assert_eq!(unknown_info, json!({}));
        assert_eq!(locked_unknown_info["current"], "9.9.9");
    }

    #[test]
    fn latest_oci_version_falls_back_to_resolved_metadata_without_exact_tags() {
        let workspace = crate::test_support::unique_temp_dir("devcontainer-catalog-test");
        let base = "ghcr.io/acme/features/moving-only";
        let digest = write_layout_version(&workspace, base, "3.0.0", None);
        replace_layout_tags(&workspace, base, &[("latest", &digest)]);

        let latest = latest_oci_version(base, Some(workspace.as_path())).expect("latest version");

        assert_eq!(latest.as_deref(), Some("3.0.0"));
        let _ = fs::remove_dir_all(workspace);
    }

    #[test]
    fn version_parsing_and_comparison_cover_selector_shapes() {
        assert_eq!(
            parse_version("1"),
            Some(ParsedVersion {
                major: 1,
                minor: 0,
                patch: 0
            })
        );
        assert_eq!(
            parse_version("1.2"),
            Some(ParsedVersion {
                major: 1,
                minor: 2,
                patch: 0
            })
        );
        assert!(parse_selector("1.2.3.4").is_none());
        assert!(parse_selector("1")
            .expect("major selector")
            .matches("1.9.0"));
        assert!(parse_selector("1.2")
            .expect("major minor selector")
            .matches("1.2.9"));
        assert!(parse_selector("1.2.3")
            .expect("exact selector")
            .matches("1.2.3"));
        assert!(!parse_selector("1.2")
            .expect("major minor selector")
            .matches("not-semver"));
        assert_eq!(major_string("2.3.4").as_deref(), Some("2"));
        assert_eq!(compare_versions_desc("beta", "alpha"), Ordering::Less);
        assert!(parse_version("1.0.0") < parse_version("2.0.0"));
    }
}
