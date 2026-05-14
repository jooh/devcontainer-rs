//! Facade for feature resolution, materialization, and metadata merge helpers.

mod control;
mod install;
mod metadata;
mod options;
mod resolve;
mod types;

pub(crate) use install::{feature_installation_name, materialize_feature_installation};
pub(crate) use metadata::apply_feature_metadata;
pub(crate) use resolve::resolve_feature_support;
pub(super) use resolve::resolve_feature_support_without_lockfile;
pub(crate) use types::{FeatureInstallation, ResolvedFeatureSupport};
