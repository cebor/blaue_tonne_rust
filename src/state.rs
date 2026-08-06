use std::sync::Arc;

use crate::cache::PdfCache;
use crate::config::Plan;
use crate::errors::PlanError;
use crate::index::{DistrictIndex, build_index};

/// The router's state: the district index, and nothing else (public so
/// integration tests can build one).
///
/// It is one field because after [`build_index`] returns, the service does no
/// I/O at all — the plans and the `reqwest::Client` that read them are gone by
/// then, and serving a request is a map lookup. The wrapper stays rather than
/// passing `Arc<DistrictIndex>` around directly so that handler and router
/// signatures name the service's state instead of one thing that happens to be
/// in it.
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
