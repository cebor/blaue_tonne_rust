//! The district index: every plan read once at startup, then served from memory.
//!
//! Plans change once a year, which is why nothing here expires and nothing is
//! fetched per request. Picking up a new `plans.yaml` — or a corrected PDF under
//! an unchanged URL — requires a restart, deliberately.

use std::collections::HashMap;
use std::time::Duration;

use bytes::Bytes;
use chrono::NaiveDate;
use reqwest::Client;

use crate::cache::PdfCache;
use crate::config::Plan;
use crate::download::download_pdf;
use crate::errors::PlanError;
use crate::pdf_parser::{index_districts, normalize_district};

/// Whole-exchange timeout for fetching one plan PDF at startup.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

/// Where a plan's bytes came from, for the one INFO line per indexed plan.
///
/// `age_secs` next to it is 0 for `url` and the file's age otherwise, so the
/// same two fields answer both "downloaded or cached?" and "how old is it?".
#[derive(Clone, Copy)]
enum PlanSource {
    Url,
    Cache,
    StaleCache,
}

impl PlanSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Url => "url",
            Self::Cache => "cache",
            Self::StaleCache => "stale-cache",
        }
    }
}

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
///
/// `cache` shortens all of this when it has the plan already, and rescues it
/// when the download fails — see the step-by-step comments below.
pub async fn build_index(plans: &[Plan], cache: &PdfCache) -> Result<DistrictIndex, PlanError> {
    // The client is local: after this function returns, the service does no
    // network I/O at all, so nothing should be able to hold on to it.
    let client = Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|e| PlanError::failed(format!("failed to build HTTP client: {e}")))?;

    let mut index = DistrictIndex::default();
    let mut plans_indexed = 0usize;

    for plan in plans {
        // Where this plan's bytes ended up coming from, reported in one INFO
        // line per plan below. One callsite rather than one per branch: an
        // operator asking "was this downloaded or read off disk?" should be able
        // to filter on a field, not to know three different message strings.
        let mut source = PlanSource::Url;
        let mut age = Duration::ZERO;

        // 1. A fresh cached copy answers the whole plan without touching the
        //    network. If it will not parse, the cache entry is bad rather than
        //    the plan — fall through and refetch, because a corrupt file would
        //    otherwise block every start from now on with an error no restart
        //    can clear.
        let mut parsed = match cache.get(&plan.url).filter(|c| c.fresh) {
            Some(cached) => match parse_plan(cached.bytes, &plan.pages).await {
                Ok(parsed) => {
                    source = PlanSource::Cache;
                    age = cached.age;
                    Some(parsed)
                }
                Err(e) => {
                    tracing::warn!(url = %plan.url, error = %e, "cached copy is unusable, refetching");
                    None
                }
            },
            None => None,
        };

        if parsed.is_none() {
            match download_pdf(&client, &plan.url).await {
                // 2. Fresh bytes. A parse error here is our own problem and
                //    stays fatal, exactly as before the cache existed. The parse
                //    runs *before* the write, so bytes that will not parse never
                //    reach the cache: otherwise the next start would find a
                //    fresh entry it has to detect as bad and throw away, and a
                //    later outage would fall back to a copy that cannot be read
                //    either.
                Ok(bytes) => {
                    parsed = Some(parse_plan(bytes.clone(), &plan.pages).await?);
                    cache.put(&plan.url, &bytes);
                }

                // 3. Gone upstream. Unchanged by the cache on purpose: 404 means
                //    the plan is retired, and serving a copy of it would keep a
                //    withdrawn plan alive for as long as the file survives.
                Err(PlanError::Retired(_)) => {
                    tracing::warn!(
                        url = %plan.url,
                        "plan is gone upstream, skipping it — prune it from plans.yaml"
                    );
                    continue;
                }

                // 4. Source unreachable. An expired copy is better than refusing
                //    to start: the dates in it were correct when they were
                //    fetched, and a plan changes once a year. Said loudly,
                //    because nothing else will report it afterwards.
                Err(e) => {
                    let stale = cache.get(&plan.url);
                    let Some(stale) = stale else { return Err(e) };

                    match parse_plan(stale.bytes, &plan.pages).await {
                        Ok(from_cache) => {
                            tracing::warn!(
                                url = %plan.url,
                                error = %e,
                                age_secs = stale.age.as_secs(),
                                "plan could not be fetched, serving a stale cached copy"
                            );
                            source = PlanSource::StaleCache;
                            age = stale.age;
                            parsed = Some(from_cache);
                        }
                        // The download failure is the cause; the unusable
                        // fallback is a consequence. Report the cause.
                        Err(parse_error) => {
                            tracing::warn!(
                                url = %plan.url,
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

        // Dates for a district that appears in several plans are concatenated in
        // plan order — one plan per year, so this is how a district keeps the
        // dates of both the old and the new plan while both are configured.
        for (district, dates) in parsed.into_iter().flatten() {
            index.dates.entry(district).or_default().extend(dates);
        }

        tracing::info!(
            url = %plan.url,
            source = source.as_str(),
            age_secs = age.as_secs(),
            districts,
            "indexed plan"
        );
        plans_indexed += 1;
    }

    // Nothing was read: every plan retired, or none configured. Serving an empty
    // index would answer "District not found" for every name without ever
    // having looked — an assertion about data we never saw.
    if plans_indexed == 0 {
        return Err(PlanError::failed(format!(
            "none of the {} configured plan(s) could be read",
            plans.len()
        )));
    }

    Ok(index)
}

/// Parse one plan's bytes off the async runtime.
///
/// Parsing is CPU-bound. Off the runtime it also turns a panic in the parser
/// into a `JoinError` instead of tearing down the process.
async fn parse_plan(
    pdf_bytes: Bytes,
    pages: &str,
) -> Result<HashMap<String, Vec<NaiveDate>>, PlanError> {
    let pages = pages.to_string();
    tokio::task::spawn_blocking(move || index_districts(&pdf_bytes, &pages))
        .await
        .map_err(|e| PlanError::failed(format!("PDF parse task failed: {e}")))?
}
