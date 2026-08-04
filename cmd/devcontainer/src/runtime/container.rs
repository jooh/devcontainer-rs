//! Native container lifecycle orchestration and shared runtime capability helpers.

mod discovery;
mod engine_run;
mod uid_update;

use std::collections::HashMap;
use std::path::Path;

use super::lifecycle::LifecycleMode;
use crate::commands::common;

pub(crate) use discovery::{
    ensure_up_container, probe_up_container_id_labels, resolve_target_container_match,
    ResolvedTargetContainer,
};
pub(crate) use engine_run::should_add_gpu_capability;
pub(crate) use engine_run::{
    contains_environment_reference, expand_environment_references, inspect_image_environment,
};
pub(crate) use uid_update::prepare_up_image;

pub(crate) struct UpContainer {
    pub(crate) container_id: String,
    pub(crate) lifecycle_mode: LifecycleMode,
    pub(crate) matched_id_labels: Option<HashMap<String, String>>,
}

pub(crate) fn dev_container_not_found_message(
    args: &[String],
    workspace_folder: Option<&Path>,
    config_file: Option<&Path>,
) -> String {
    if common::parse_option_values(args, "--id-label").is_empty() {
        if let (Some(workspace_folder), Some(config_file)) = (workspace_folder, config_file) {
            return format!(
                "Dev container not found for workspace folder '{}' and config file '{}'. If the container was created with a different config file, pass --config <path> or set DEVCONTAINER_CONFIG.",
                workspace_folder.display(),
                config_file.display()
            );
        }
    }
    "Dev container not found.".to_string()
}
