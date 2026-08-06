use std::sync::Arc;

use crate::cache::PdfCache;
use crate::config::Plan;
use crate::errors::PlanError;
use crate::index::{DistrictIndex, build_index};

/// Shared application state (public so integration tests can build it).
///
/// Nothing in here changes while the process runs: every plan is read once at
/// startup, so serving a request is a map lookup and no more.
#[derive(Clone)]
pub struct AppState {
    pub index: Arc<DistrictIndex>,
}

impl AppState {
    /// Read every configured plan and build the index. See [`build_index`] for
    /// what makes this fail — it is meant to be fatal at startup.
    pub async fn build(plans: &[Plan], cache: &PdfCache) -> Result<Self, PlanError> {
        Ok(Self::from_index(build_index(plans, cache).await?))
    }

    pub fn from_index(index: DistrictIndex) -> Self {
        Self {
            index: Arc::new(index),
        }
    }
}
