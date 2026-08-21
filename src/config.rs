use ipnet::IpNet;
use serde::Deserialize;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `plans` is a list of PDF URLs and nothing else — `index_districts` reads
/// every page of a plan, so there is no per-plan setting left to carry.
/// `deny_unknown_fields` keeps a stray top-level key from being ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    plans: Vec<String>,
}

/// Reject a plan URL that `download_pdf` could never fetch: scheme must be
/// `http`/`https`, and the URL *path* must end in `.pdf`.
///
/// Matching on the path rather than the whole string lets a link carry a query
/// string or fragment (`…/Abfuhrplan_2027.pdf?v=2`). This is the only place the
/// rule lives; `download.rs` does not repeat it.
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

/// Read `plans.yaml` into the list of plan PDF URLs, every one of them validated.
pub fn load_plans(path: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let config: Config = serde_yaml_ng::from_str(&content)?;
    for url in &config.plans {
        validate_plan_url(url)?;
    }
    Ok(config.plans)
}

/// The two default routes, which together cover every address. `*` expands to
/// these, so trusting every peer stays a value in the allowlist rather than a
/// special case `resolve_client_ip` has to know about.
const TRUST_EVERY_PEER: [IpNet; 2] = [
    IpNet::new_assert(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
    IpNet::new_assert(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
];

/// Parse the `FORWARDED_ALLOW_IPS` value into a list of trusted networks.
///
/// Accepts both plain IPs (`"127.0.0.1"`) and CIDR notation (`"10.0.0.0/8"`),
/// comma-separated. Whitespace is trimmed and empty entries are ignored.
/// Invalid entries are logged via `tracing::warn!` and skipped.
///
/// A `*` entry trusts every peer and makes the rest of the list redundant, so it
/// short-circuits to [`TRUST_EVERY_PEER`]. Startup logs the two networks rather
/// than the `*` that produced them.
pub fn parse_forwarded_allow_ips(raw: &str) -> Vec<IpNet> {
    if raw.split(',').any(|entry| entry.trim() == "*") {
        return TRUST_EVERY_PEER.to_vec();
    }

    raw.split(',')
        .filter_map(|s| {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            s.parse::<IpNet>()
                .or_else(|_| s.parse::<std::net::IpAddr>().map(IpNet::from))
                .map_err(|e| {
                    tracing::warn!("Ignoring invalid FORWARDED_ALLOW_IPS entry {s:?}: {e}")
                })
                .ok()
        })
        .collect()
}

/// The URL the `healthcheck` subcommand probes, derived from `BIND_ADDR`.
///
/// An unspecified bind address (`0.0.0.0`, `[::]`) is not an address to connect
/// to, so it becomes the matching loopback; `SocketAddr`'s `Display` brackets
/// IPv6. Anything that is not a socket address — a hostname — is used as given.
pub fn healthcheck_url(bind_addr: &str) -> String {
    let host = match bind_addr.parse::<SocketAddr>() {
        Ok(addr) if addr.ip().is_unspecified() => {
            let loopback: IpAddr = match addr.ip() {
                IpAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
                IpAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
            };
            SocketAddr::new(loopback, addr.port()).to_string()
        }
        _ => bind_addr.to_string(),
    };

    format!("http://{host}/health")
}

/// How long a cached plan PDF counts as fresh when `PDF_CACHE_TTL` is unset.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Directory name appended to whichever cache root is in effect.
const CACHE_SUBDIR: &str = "blaue_tonne_rust";

/// Parse `PDF_CACHE_TTL` — `30d`, `12h`, `90m`, `45s`, or a bare number of
/// seconds. Bad input warns and falls back to [`DEFAULT_CACHE_TTL`].
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
/// `PDF_CACHE_DIR` **unset** picks the default location; **set to an empty
/// value** turns the cache off.
///
/// A pure function of its three inputs so it is testable without mutating
/// process-wide environment variables.
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
        // Last resort: always writable, but may be wiped between starts.
        .unwrap_or_else(std::env::temp_dir);

    Some(root.join(CACHE_SUBDIR))
}

/// [`cache_dir_from`] applied to the real environment.
pub(crate) fn cache_dir_from_env() -> Option<PathBuf> {
    cache_dir_from(
        std::env::var("PDF_CACHE_DIR").ok().as_deref(),
        std::env::var("XDG_CACHE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
}
