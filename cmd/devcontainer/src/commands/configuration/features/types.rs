//! Shared data structures for feature resolution, metadata, and installation materialization.

use std::path::PathBuf;

use serde_json::Value;

use crate::commands::collections::oci::OciFeatureArtifact;

#[derive(Clone, Debug)]
pub(crate) enum FeatureInstallationSource {
    Local(PathBuf),
    Published(Box<OciFeatureArtifact>),
    DirectTarball(String),
    GithubRepo(String),
}

#[derive(Clone, Debug)]
pub(crate) struct FeatureInstallation {
    pub(crate) source: FeatureInstallationSource,
    pub(crate) env: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedFeatureSupport {
    pub(crate) features_configuration: Value,
    pub(crate) feature_advisories: Vec<Value>,
    pub(crate) metadata_entries: Vec<Value>,
    pub(crate) installations: Vec<FeatureInstallation>,
    pub(crate) ordered_features: Vec<ResolvedFeatureSummary>,
    pub(crate) ordered_feature_ids: Vec<String>,
    pub(crate) lockfile_features: Vec<ResolvedLockfileFeature>,
}

#[derive(Clone)]
pub(super) struct FeatureSpec {
    pub(super) user_feature_id: String,
    pub(super) manifest: Value,
    pub(super) options: Value,
    pub(super) value: Value,
    pub(super) source_information: Value,
    pub(super) metadata_entry: Value,
    pub(super) installation: FeatureInstallation,
    pub(super) install_order_id: String,
    pub(super) source: FeatureSource,
    pub(super) aliases: Vec<String>,
    pub(super) depends_on: Vec<FeatureRequest>,
    pub(super) installs_after: Vec<FeatureRequest>,
    pub(super) lockfile_feature: Option<ResolvedLockfileFeature>,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedFeatureSummary {
    pub(crate) id: String,
    pub(crate) options: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedLockfileFeature {
    pub(crate) user_feature_id: String,
    pub(crate) version: String,
    pub(crate) resolved: String,
    pub(crate) integrity: String,
    pub(crate) depends_on: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub(super) struct FeatureRequest {
    pub(super) user_feature_id: String,
    pub(super) options: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FeatureSource {
    Local {
        resolved_path: String,
    },
    Oci {
        resource: String,
        tag: Option<String>,
        digest: String,
    },
    DirectTarball {
        uri: String,
    },
    GithubRepo {
        id_without_version: String,
    },
}
