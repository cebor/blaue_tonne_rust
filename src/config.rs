use ipnet::IpNet;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct Plan {
    pub url: String,
    pub pages: String,
}

#[derive(Debug, Deserialize)]
struct Config {
    plans: Vec<Plan>,
}

/// Reject a plan URL that `download_pdf` could never fetch.
///
/// Whether a URL is fetchable at all is a property of the config, not of the
/// plan behind it. Checking it here makes a typo in `plans.yaml` fail while the
/// config is being read, naming the offending URL; left to `download_pdf` it
/// would still be fatal — every plan fault at startup is — but it would surface
/// as a failed fetch halfway through the index build, blaming the source for
/// something we wrote ourselves.
///
/// The `.pdf` check is on the URL *path*, so a query string or fragment on an
/// otherwise valid link (`…/Abfuhrplan_2027.pdf?v=2`) is not rejected.
fn validate_plan_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid plan URL {url:?}: {e}"))?;

    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "plan URL {url:?} must use http or https, got {:?}",
            parsed.scheme()
        ));
    }

    if !parsed.path().to_lowercase().ends_with(".pdf") {
        return Err(format!("plan URL {url:?} must point at a .pdf path"));
    }

    Ok(())
}

pub fn load_plans(path: &Path) -> Result<Vec<Plan>, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let config: Config = serde_yaml_ng::from_str(&content)?;
    for plan in &config.plans {
        validate_plan_url(&plan.url)?;
    }
    Ok(config.plans)
}

/// Parse the `FORWARDED_ALLOW_IPS` value into a list of trusted networks.
///
/// Accepts both plain IPs (`"127.0.0.1"`) and CIDR notation (`"10.0.0.0/8"`),
/// comma-separated. Whitespace is trimmed and empty entries are ignored.
/// Invalid entries are logged via `tracing::warn!` and skipped.
pub fn parse_forwarded_allow_ips(raw: &str) -> Vec<IpNet> {
    raw.split(',')
        .filter_map(|s| {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            // Accept both plain IPs ("127.0.0.1") and CIDR notation ("10.0.0.0/8").
            s.parse::<IpNet>()
                .or_else(|_| s.parse::<std::net::IpAddr>().map(IpNet::from))
                .map_err(|e| {
                    tracing::warn!("Ignoring invalid FORWARDED_ALLOW_IPS entry {s:?}: {e}")
                })
                .ok()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// PDF cache configuration
// ---------------------------------------------------------------------------

/// How long a cached plan PDF counts as fresh when `PDF_CACHE_TTL` is unset.
///
/// One month: the plans themselves change once a year, so this is not about
/// catching a new plan (a restart does that) but about not serving bytes nobody
/// has re-checked in an unbounded amount of time.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Directory name appended to whichever cache root is in effect.
const CACHE_SUBDIR: &str = "blaue_tonne_rust";

/// Parse `PDF_CACHE_TTL` — `30d`, `12h`, `90m`, `45s`, or a bare number of
/// seconds.
///
/// Like [`parse_forwarded_allow_ips`], bad input warns and falls back rather
/// than failing: a typo in a cache knob must not keep the service from starting,
/// and the fallback is a working default, not a guess at what was meant.
///
/// `0` is legal and means "never fresh": every plan is re-downloaded, but the
/// cache is still written, so the stale fallback in `build_index` keeps working.
pub fn parse_cache_ttl(raw: &str) -> Duration {
    let raw = raw.trim();
    if raw.is_empty() {
        return DEFAULT_CACHE_TTL;
    }

    let (digits, multiplier) = match raw.as_bytes().last() {
        Some(b's') => (&raw[..raw.len() - 1], 1),
        Some(b'm') => (&raw[..raw.len() - 1], 60),
        Some(b'h') => (&raw[..raw.len() - 1], 60 * 60),
        Some(b'd') => (&raw[..raw.len() - 1], 24 * 60 * 60),
        _ => (raw, 1),
    };

    match digits.parse::<u64>() {
        Ok(n) => match n.checked_mul(multiplier) {
            Some(secs) => Duration::from_secs(secs),
            None => {
                tracing::warn!("PDF_CACHE_TTL {raw:?} overflows, using the default");
                DEFAULT_CACHE_TTL
            }
        },
        Err(e) => {
            tracing::warn!("Ignoring invalid PDF_CACHE_TTL {raw:?}: {e}");
            DEFAULT_CACHE_TTL
        }
    }
}

/// Resolve the PDF cache directory from the three environment values that can
/// determine it. `None` means the cache is switched off.
///
/// Split out from [`cache_dir_from_env`] so it can be tested without mutating
/// process-wide environment variables, which is not safe while other tests run
/// in parallel.
///
/// The distinction that matters: `PDF_CACHE_DIR` **unset** picks the default
/// location, `PDF_CACHE_DIR` **set to an empty value** turns the cache off. One
/// variable therefore covers both the path and the off switch, and there is no
/// way to configure a contradiction.
pub fn cache_dir_from(
    cache_dir: Option<&str>,
    xdg_cache_home: Option<&str>,
    home: Option<&str>,
) -> Option<PathBuf> {
    if let Some(explicit) = cache_dir {
        let explicit = explicit.trim();
        return (!explicit.is_empty()).then(|| PathBuf::from(explicit));
    }

    let root = xdg_cache_home
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|h| PathBuf::from(h).join(".cache"))
        })
        // Always writable, but wiped often enough that the cache may simply be
        // cold. Better than no cache at all, and better than failing to start
        // for want of a home directory.
        .unwrap_or_else(std::env::temp_dir);

    Some(root.join(CACHE_SUBDIR))
}

/// [`cache_dir_from`] applied to the real environment.
pub fn cache_dir_from_env() -> Option<PathBuf> {
    cache_dir_from(
        std::env::var("PDF_CACHE_DIR").ok().as_deref(),
        std::env::var("XDG_CACHE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}
