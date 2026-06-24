use std::io::Write;
use std::path::PathBuf;

use blaue_tonne_rust::config::{load_plans, parse_forwarded_allow_ips};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Write `content` to a uniquely-named temp file and return its path.
fn write_temp(name: &str, content: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "blaue_tonne_test_{}_{}_{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut f = std::fs::File::create(&path).expect("create temp file");
    f.write_all(content.as_bytes()).expect("write temp file");
    path
}

// ---------------------------------------------------------------------------
// load_plans
// ---------------------------------------------------------------------------

#[test]
fn test_load_plans_success() {
    let yaml = r#"
plans:
  - url: "https://example.test/a.pdf"
    pages: "1,2"
  - url: "https://example.test/b.pdf"
    pages: "3"
"#;
    let path = write_temp("plans_ok", yaml);
    let plans = load_plans(&path).expect("should parse");
    std::fs::remove_file(&path).ok();

    assert_eq!(plans.len(), 2);
    assert_eq!(plans[0].url, "https://example.test/a.pdf");
    assert_eq!(plans[0].pages, "1,2");
    assert_eq!(plans[1].url, "https://example.test/b.pdf");
    assert_eq!(plans[1].pages, "3");
}

#[test]
fn test_load_plans_missing_file_errors() {
    let path = PathBuf::from("/nonexistent/path/does_not_exist_plans.yaml");
    assert!(load_plans(&path).is_err());
}

#[test]
fn test_load_plans_invalid_yaml_errors() {
    let path = write_temp("plans_bad", "this: is: not: valid: yaml: [[[");
    let result = load_plans(&path);
    std::fs::remove_file(&path).ok();
    assert!(result.is_err());
}

#[test]
fn test_load_plans_missing_plans_key_errors() {
    // Valid YAML but missing the required `plans` field.
    let path = write_temp("plans_nokey", "something_else: 42\n");
    let result = load_plans(&path);
    std::fs::remove_file(&path).ok();
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// parse_forwarded_allow_ips
// ---------------------------------------------------------------------------

#[test]
fn test_parse_allow_ips_empty() {
    assert!(parse_forwarded_allow_ips("").is_empty());
    assert!(parse_forwarded_allow_ips("   ").is_empty());
    assert!(parse_forwarded_allow_ips(",,").is_empty());
}

#[test]
fn test_parse_allow_ips_single_ip() {
    let nets = parse_forwarded_allow_ips("127.0.0.1");
    assert_eq!(nets.len(), 1);
    assert!(nets[0].contains(&"127.0.0.1".parse::<std::net::IpAddr>().unwrap()));
}

#[test]
fn test_parse_allow_ips_cidr() {
    let nets = parse_forwarded_allow_ips("10.0.0.0/8");
    assert_eq!(nets.len(), 1);
    assert!(nets[0].contains(&"10.1.2.3".parse::<std::net::IpAddr>().unwrap()));
    assert!(!nets[0].contains(&"11.0.0.1".parse::<std::net::IpAddr>().unwrap()));
}

#[test]
fn test_parse_allow_ips_mixed_with_whitespace() {
    let nets = parse_forwarded_allow_ips(" 127.0.0.1 , 10.0.0.0/8 ,, 192.168.1.5 ");
    assert_eq!(nets.len(), 3);
}

#[test]
fn test_parse_allow_ips_skips_invalid_entries() {
    // Invalid entries (including "*") are logged and skipped; valid ones remain.
    let nets = parse_forwarded_allow_ips("127.0.0.1, not-an-ip, *, 10.0.0.0/8");
    assert_eq!(nets.len(), 2);
}

#[test]
fn test_parse_allow_ips_ipv6() {
    let nets = parse_forwarded_allow_ips("::1");
    assert_eq!(nets.len(), 1);
    assert!(nets[0].contains(&"::1".parse::<std::net::IpAddr>().unwrap()));
}
