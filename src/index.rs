//! The district index: every plan read once at startup, then served from memory.
//!
//! Nothing expires and nothing is fetched per request. A new `plans.yaml` — or a
//! corrected PDF under an unchanged URL — is picked up on restart only.

use std::collections::HashMap;
use std::time::Duration;

use bytes::Bytes;
use chrono::NaiveDate;
use reqwest::Client;

use crate::cache::PdfCache;
use crate::download::download_pdf;
use crate::errors::PlanError;
use crate::pdf_parser::{District, index_districts, normalize_district};

/// Whole-exchange timeout for fetching one plan PDF at startup.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// All known districts and their collection dates. Immutable once built.
#[derive(Debug, Default)]
pub struct DistrictIndex {
    districts: HashMap<String, District>,
}

impl DistrictIndex {
    /// Look a district up by its [`normalize_district`]ed name.
    pub fn lookup(&self, normalized_district: &str) -> Option<&[NaiveDate]> {
        self.districts
            .get(normalized_district)
            .map(|d| d.dates.as_slice())
    }

    /// Every district's printed name, sorted — the list `/docs` offers.
    ///
    /// Sorted by code point, so umlaut initials land after `Z`. Deterministic is
    /// what a dropdown needs; collation is not worth a dependency.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.districts.values().map(|d| d.name.clone()).collect();
        names.sort();
        names
    }

    pub fn len(&self) -> usize {
        self.districts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.districts.is_empty()
    }

    /// Build an index from ready-made entries, for tests that want a known set
    /// of districts without a PDF. Normalizes the keys, so a caller cannot seed
    /// an entry that `lookup` could never reach; the name as given stays the
    /// printed one.
    pub fn from_pairs(entries: impl IntoIterator<Item = (String, Vec<NaiveDate>)>) -> Self {
        Self {
            districts: entries
                .into_iter()
                .map(|(name, dates)| (normalize_district(&name), District { name, dates }))
                .collect(),
        }
    }
}

/// Download and parse every configured plan into one index.
///
/// Fails on the first plan that cannot be read, except for a plan whose PDF is
/// gone upstream (HTTP 404): that one is skipped with a WARN. Indexing no plan
/// at all is fatal.
///
/// `cache` is consulted before every download, and is used as a fallback when a
/// download fails.
pub async fn build_index(
    plan_urls: &[String],
    cache: &PdfCache,
) -> Result<DistrictIndex, PlanError> {
    // Local to the build: after this returns, the service does no network I/O.
    let client = Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|e| PlanError::failed(format!("failed to build HTTP client: {e}")))?;

    let mut index = DistrictIndex::default();
    let mut plans_indexed = 0usize;

    for url in plan_urls {
        // Reported as fields on the single INFO line at the end of the loop
        // body. `age` is zero for `url` and the file's age otherwise.
        let mut source = "url";
        let mut age = Duration::ZERO;

        // A fresh cached copy answers the plan without touching the network. If
        // it will not parse, the entry is bad rather than the plan: refetch.
        let mut parsed = match cache.get(url).filter(|c| c.fresh) {
            Some(cached) => match parse_plan(cached.bytes).await {
                Ok(parsed) => {
                    source = "cache";
                    age = cached.age;
                    Some(parsed)
                }
                Err(e) => {
                    tracing::warn!(%url, error = %e, "cached copy is unusable, refetching");
                    None
                }
            },
            None => None,
        };

        if parsed.is_none() {
            match download_pdf(&client, url).await {
                // Parse before write, so bytes that will not parse never reach
                // the cache.
                Ok(bytes) => {
                    parsed = Some(parse_plan(bytes.clone()).await?);
                    cache.put(url, &bytes);
                }

                // 404 means retired, so the cache is not consulted: a copy would
                // keep a withdrawn plan alive for as long as the file survives.
                Err(PlanError::Retired(_)) => {
                    tracing::warn!(
                        %url,
                        "plan is gone upstream, skipping it — prune it from plans.yaml"
                    );
                    continue;
                }

                // Source unreachable: fall back to an expired copy, with a WARN
                // as the only signal that this happened.
                Err(e) => {
                    let Some(stale) = cache.get(url) else {
                        return Err(e);
                    };

                    match parse_plan(stale.bytes).await {
                        Ok(from_cache) => {
                            tracing::warn!(
                                %url,
                                error = %e,
                                age_secs = stale.age.as_secs(),
                                "plan could not be fetched, serving a stale cached copy"
                            );
                            source = "stale-cache";
                            age = stale.age;
                            parsed = Some(from_cache);
                        }
                        // Report the download error, not the parse error: the
                        // failed fetch is the cause.
                        Err(parse_error) => {
                            tracing::warn!(
                                %url,
                                error = %parse_error,
                                "stale cached copy is unusable too"
                            );
                            return Err(e);
                        }
                    }
                }
            }
        }

        let districts = parsed.iter().flatten().count();

        // Dates for a district several plans carry are concatenated in plan
        // order — not deduplicated, not sorted. The first plan's spelling of the
        // name is the one kept.
        for (key, district) in parsed.into_iter().flatten() {
            index
                .districts
                .entry(key)
                .or_insert_with(|| District {
                    name: district.name,
                    dates: Vec::new(),
                })
                .dates
                .extend(district.dates);
        }

        tracing::info!(
            %url,
            source,
            age_secs = age.as_secs(),
            districts,
            "indexed plan"
        );
        plans_indexed += 1;
    }

    // Every plan retired, or none configured. An empty index would answer
    // "District not found" for every name in the county.
    if plans_indexed == 0 {
        return Err(PlanError::failed(format!(
            "none of the {} configured plan(s) could be read",
            plan_urls.len()
        )));
    }

    Ok(index)
}

/// Parse one plan's bytes on a blocking thread.
///
/// `spawn_blocking` for the panic, not for the runtime: `pdf_oxide` is fed
/// third-party bytes and a panic in it arrives here as a `JoinError`, becoming a
/// `PlanError` instead of a backtrace on stderr and exit 101.
async fn parse_plan(pdf_bytes: Bytes) -> Result<HashMap<String, District>, PlanError> {
    tokio::task::spawn_blocking(move || index_districts(&pdf_bytes))
        .await
        .map_err(|e| PlanError::failed(format!("PDF parse task failed: {e}")))?
}
