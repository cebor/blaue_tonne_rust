use ipnet::IpNet;
use serde::Deserialize;
use std::fs;
use std::path::Path;

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
/// request. Checking it here makes a typo in `plans.yaml` fail once, at startup;
/// left to `download_pdf` it becomes a 503 ("try again later" — it never will)
/// plus a WARN on every request for the lifetime of the process.
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
