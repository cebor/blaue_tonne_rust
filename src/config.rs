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

pub fn load_plans(path: &Path) -> Result<Vec<Plan>, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let config: Config = serde_yaml::from_str(&content)?;
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
                .map_err(|e| tracing::warn!("Ignoring invalid FORWARDED_ALLOW_IPS entry {s:?}: {e}"))
                .ok()
        })
        .collect()
}
