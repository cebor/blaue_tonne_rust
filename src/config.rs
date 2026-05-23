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
