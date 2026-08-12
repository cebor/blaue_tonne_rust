use std::sync::Arc;

use crate::cache::PdfCache;
use crate::config::Plan;
use crate::errors::PlanError;
use crate::index::{DistrictIndex, build_index};

/// The router's state: the district index, and nothing else. After
/// [`build_index`] returns, serving a request is a map lookup and no more.
#[derive(Clone)]
pub struct AppState {
    pub index: Arc<DistrictIndex>,
}

impl AppState {
    /// Read every configured plan and build the index. Fatal at startup when it
    /// fails; see [`build_index`].
    pub async fn build(plans: &[Plan], cache: &PdfCache) -> Result<Self, PlanError> {
        Ok(Self::from_index(build_index(plans, cache).await?))
    }

    pub fn from_index(index: DistrictIndex) -> Self {
        Self {
            index: Arc::new(index),
        }
    }
}
