//! The district index: every plan read once at startup, then served from memory.
//!
//! Plans change once a year, which is why nothing here expires and nothing is
//! fetched per request. Picking up a new `plans.yaml` — or a corrected PDF under
//! an unchanged URL — requires a restart, deliberately.

use std::collections::HashMap;

use chrono::NaiveDate;
use reqwest::Client;

use crate::config::Plan;
use crate::download::download_pdf;
use crate::errors::AppError;
use crate::pdf_parser::{index_districts, normalize_district};

/// Whole-exchange timeout for fetching one plan PDF at startup.
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// All known districts and their collection dates. Immutable once built.
#[derive(Debug, Default)]
pub struct DistrictIndex {
    dates: HashMap<String, Vec<NaiveDate>>,
}

impl DistrictIndex {
    /// Look a district up by its [`normalize_district`]ed name.
    pub fn lookup(&self, normalized_district: &str) -> Option<&[NaiveDate]> {
        self.dates.get(normalized_district).map(Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.dates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.dates.is_empty()
    }

    /// Build an index from ready-made entries, for tests that want a known set
    /// of districts without a PDF. Normalizes the keys itself so a caller
    /// cannot seed an entry that `lookup` could never reach.
    pub fn from_pairs(entries: impl IntoIterator<Item = (String, Vec<NaiveDate>)>) -> Self {
        Self {
            dates: entries
                .into_iter()
                .map(|(district, dates)| (normalize_district(&district), dates))
                .collect(),
        }
    }
}

/// Download and parse every configured plan into one index.
///
/// Fails on the first plan that cannot be read. There is no second chance at
/// request time any more, so a half-built index would serve some districts
/// short of their dates for the lifetime of the process — refusing to start is
/// the honest outcome, and it is visible in the restart rather than silent in
/// the data.
///
/// The one exception is a plan whose PDF is gone upstream (HTTP 404). That is
/// expected at the turn of the year, when last year's plan goes offline while it
/// is still listed in `plans.yaml`, and it must not keep the service down until
/// someone prunes the config. Such a plan is skipped with a WARN — once, at
/// startup. Only if *no* plan could be indexed at all does that become fatal.
pub async fn build_index(plans: &[Plan]) -> Result<DistrictIndex, AppError> {
    // The client is local: after this function returns, the service does no
    // network I/O at all, so nothing should be able to hold on to it.
    let client = Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|e| AppError::Upstream(format!("failed to build HTTP client: {e}")))?;

    let mut index = DistrictIndex::default();
    let mut plans_indexed = 0usize;

    for plan in plans {
        let pdf_bytes = match download_pdf(&client, &plan.url).await {
            Ok(bytes) => bytes,
            Err(AppError::PdfNotFound(_)) => {
                tracing::warn!(
                    url = %plan.url,
                    "plan is gone upstream, skipping it — prune it from plans.yaml"
                );
                continue;
            }
            Err(e) => return Err(e),
        };

        // Parsing is CPU-bound. Off the runtime it also turns a panic in the
        // parser into a JoinError instead of tearing down the process.
        let pages = plan.pages.clone();
        let parsed = tokio::task::spawn_blocking(move || index_districts(&pdf_bytes, &pages))
            .await
            .map_err(|e| AppError::PdfError(format!("PDF parse task failed: {e}")))??;

        // Dates for a district that appears in several plans are concatenated in
        // plan order — one plan per year, so this is how a district keeps the
        // dates of both the old and the new plan while both are configured.
        for (district, dates) in parsed {
            index.dates.entry(district).or_default().extend(dates);
        }

        plans_indexed += 1;
        tracing::info!(url = %plan.url, "indexed plan");
    }

    // Nothing was read: every plan retired, or none configured. Serving an empty
    // index would answer "District not found" for every name without ever
    // having looked — an assertion about data we never saw.
    if plans_indexed == 0 {
        return Err(AppError::Upstream(format!(
            "none of the {} configured plan(s) could be read",
            plans.len()
        )));
    }

    Ok(index)
}
