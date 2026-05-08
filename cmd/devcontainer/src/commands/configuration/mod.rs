//! Configuration command entrypoints and shared configuration helpers.

mod catalog;
mod features;
mod inspect;
mod load;
mod merge;
mod read;
mod upgrade;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::ExitCode;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) use features::FeatureInstallation;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Lockfile {
    features: BTreeMap<String, LockfileEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct LockfileEntry {
    version: String,
    resolved: String,
    integrity: String,
    #[serde(rename = "dependsOn", skip_serializing_if = "Option::is_none")]
    depends_on: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CatalogEntry {
    version: String,
    resolved: String,
    integrity: String,
    depends_on: Option<Vec<String>>,
}

struct LoadedConfig {
    workspace_folder: PathBuf,
    config_file: PathBuf,
    raw_text: String,
    configuration: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ParsedVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

#[derive(Clone, Debug)]
struct FeatureReference {
    original: String,
    base: String,
    tag: Option<String>,
    digest: Option<String>,
}

struct InspectedContainer {
    metadata_entries: Vec<Value>,
    container_env: HashMap<String, String>,
}

pub(crate) fn build_read_configuration_payload(args: &[String]) -> Result<Value, String> {
    read::build_read_configuration_payload(args)
}

pub(crate) fn resolve_feature_support(
    args: &[String],
    workspace_folder: &std::path::Path,
    config_file: &std::path::Path,
    configuration: &Value,
) -> Result<Option<features::ResolvedFeatureSupport>, String> {
    features::resolve_feature_support(args, workspace_folder, config_file, configuration)
}

pub(crate) fn materialize_feature_installation(
    installation: &features::FeatureInstallation,
    destination: &std::path::Path,
) -> Result<(), String> {
    features::materialize_feature_installation(installation, destination)
}

pub(crate) fn feature_installation_name(installation: &features::FeatureInstallation) -> String {
    features::feature_installation_name(installation)
}

pub(crate) fn apply_feature_metadata(configuration: &Value, metadata_entries: &[Value]) -> Value {
    features::apply_feature_metadata(configuration, metadata_entries, false)
}

pub(crate) fn apply_feature_metadata_with_options(
    configuration: &Value,
    metadata_entries: &[Value],
    skip_feature_customizations: bool,
) -> Value {
    features::apply_feature_metadata(configuration, metadata_entries, skip_feature_customizations)
}

pub(crate) fn ensure_native_lockfile(
    args: &[String],
    config_file: &std::path::Path,
    configuration: &Value,
) -> Result<(), String> {
    upgrade::ensure_native_lockfile(args, config_file, configuration)
}

pub(crate) fn should_use_native_read_configuration(args: &[String]) -> bool {
    read::should_use_native_read_configuration(args)
}

pub(crate) fn run_outdated(args: &[String]) -> ExitCode {
    upgrade::run_outdated(args)
}

pub(crate) fn run_upgrade(args: &[String]) -> ExitCode {
    upgrade::run_upgrade(args)
}

#[cfg(test)]
mod tests;
