use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use blaue_tonne_rust::config::{
    DEFAULT_CACHE_TTL, cache_dir_from, load_plans, parse_cache_ttl, parse_forwarded_allow_ips,
};

// --- Helpers ---
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

// --- load_plans ---
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

// --- Plan URL validation ---
#[test]
fn test_load_plans_rejects_non_pdf_url() {
    let yaml = r#"
plans:
  - url: "https://example.test/schedule.html"
    pages: "1"
"#;
    let path = write_temp("plans_not_pdf", yaml);
    let result = load_plans(&path);
    std::fs::remove_file(&path).ok();
    assert!(
        result.is_err(),
        "a non-.pdf plan URL must fail at load time"
    );
}

#[test]
fn test_load_plans_rejects_non_http_scheme() {
    let yaml = r#"
plans:
  - url: "file:///etc/schedule.pdf"
    pages: "1"
"#;
    let path = write_temp("plans_bad_scheme", yaml);
    let result = load_plans(&path);
    std::fs::remove_file(&path).ok();
    assert!(result.is_err());
}

#[test]
fn test_load_plans_rejects_unparseable_url() {
    let yaml = r#"
plans:
  - url: "not a url at all.pdf"
    pages: "1"
"#;
    let path = write_temp("plans_unparseable", yaml);
    let result = load_plans(&path);
    std::fs::remove_file(&path).ok();
    assert!(result.is_err());
}

#[test]
fn test_load_plans_accepts_pdf_url_with_query() {
    // The check is on the path, so a cache-busting query is not a config error.
    let yaml = r#"
plans:
  - url: "https://example.test/Abfuhrplan_2027.PDF?v=2#page=3"
    pages: "1"
"#;
    let path = write_temp("plans_query", yaml);
    let plans = load_plans(&path).expect("a query string must not disqualify the URL");
    std::fs::remove_file(&path).ok();
    assert_eq!(plans.len(), 1);
}

// --- parse_forwarded_allow_ips ---
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

// --- parse_cache_ttl ---
#[test]
fn test_parse_cache_ttl_accepts_every_suffix() {
    assert_eq!(parse_cache_ttl("45s"), Duration::from_secs(45));
    assert_eq!(parse_cache_ttl("90m"), Duration::from_secs(90 * 60));
    assert_eq!(parse_cache_ttl("12h"), Duration::from_secs(12 * 3600));
    assert_eq!(parse_cache_ttl("30d"), Duration::from_secs(30 * 86_400));
    assert_eq!(parse_cache_ttl(" 7d "), Duration::from_secs(7 * 86_400));
}

#[test]
fn test_parse_cache_ttl_treats_a_bare_number_as_seconds() {
    assert_eq!(parse_cache_ttl("3600"), Duration::from_secs(3600));
}

#[test]
fn test_parse_cache_ttl_zero_is_legal() {
    // Zero means "never fresh", which still writes the cache.
    assert_eq!(parse_cache_ttl("0"), Duration::ZERO);
    assert_eq!(parse_cache_ttl("0d"), Duration::ZERO);
}

#[test]
fn test_parse_cache_ttl_falls_back_on_nonsense() {
    // A typo in a cache knob must not keep the service from starting.
    for raw in [
        "",
        "   ",
        "soon",
        "1w",
        "-5",
        "5.5h",
        "99999999999999999999d",
    ] {
        assert_eq!(
            parse_cache_ttl(raw),
            DEFAULT_CACHE_TTL,
            "{raw:?} should have fallen back"
        );
    }
}

#[test]
fn test_default_cache_ttl_is_a_month() {
    assert_eq!(DEFAULT_CACHE_TTL, Duration::from_secs(30 * 86_400));
}

// --- cache_dir_from ---
#[test]
fn test_cache_dir_prefers_an_explicit_path() {
    assert_eq!(
        cache_dir_from(Some("/var/cache/x"), Some("/xdg"), Some("/home/u")),
        Some(PathBuf::from("/var/cache/x"))
    );
}

#[test]
fn test_an_empty_cache_dir_switches_the_cache_off() {
    // *Unset* means the default location, *set but empty* means off.
    assert_eq!(
        cache_dir_from(Some(""), Some("/xdg"), Some("/home/u")),
        None
    );
    assert_eq!(cache_dir_from(Some("  "), None, None), None);
    assert!(cache_dir_from(None, Some("/xdg"), Some("/home/u")).is_some());
}

#[test]
fn test_cache_dir_falls_back_xdg_then_home_then_temp() {
    assert_eq!(
        cache_dir_from(None, Some("/xdg"), Some("/home/u")),
        Some(PathBuf::from("/xdg/blaue_tonne_rust"))
    );
    assert_eq!(
        cache_dir_from(None, None, Some("/home/u")),
        Some(PathBuf::from("/home/u/.cache/blaue_tonne_rust"))
    );
    assert_eq!(
        cache_dir_from(None, Some(""), Some("")),
        Some(std::env::temp_dir().join("blaue_tonne_rust"))
    );
}
