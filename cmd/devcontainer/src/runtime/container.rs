//! Native container lifecycle orchestration and shared runtime capability helpers.

mod discovery;
mod engine_run;
mod uid_update;

use std::collections::HashMap;

use super::lifecycle::LifecycleMode;

pub(crate) use discovery::{
    ensure_up_container, probe_up_container_id_labels, resolve_target_container_match,
    ResolvedTargetContainer,
};
pub(crate) use engine_run::should_add_gpu_capability;
pub(crate) use uid_update::prepare_up_image;

pub(crate) struct UpContainer {
    pub(crate) container_id: String,
    pub(crate) lifecycle_mode: LifecycleMode,
    pub(crate) matched_id_labels: Option<HashMap<String, String>>,
}
