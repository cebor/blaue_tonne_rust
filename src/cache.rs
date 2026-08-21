//! On-disk cache for the plan PDFs.
//!
//! Used only from [`crate::index::build_index`], before the listener binds —
//! hence blocking `std::fs` rather than tokio's `fs` feature.
//!
//! Every failure here degrades to "no cache" plus a log line; nothing in this
//! module can make the service fail.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use sha2::{Digest, Sha256};

use crate::config::{cache_dir_from_env, parse_cache_ttl};

/// A copy read back from the cache.
pub struct CachedPdf {
    pub bytes: Bytes,
    /// Whether the file is still within the configured TTL.
    pub fresh: bool,
    /// How long ago the file was written. Zero if the mtime is unreadable or in
    /// the future.
    pub age: Duration,
}

/// A directory of downloaded plan PDFs, keyed by URL.
///
/// `dir: None` is a disabled cache: `get` and `put` are no-ops, so callers need
/// no branch of their own.
pub struct PdfCache {
    dir: Option<PathBuf>,
    ttl: Duration,
}

impl PdfCache {
    /// Read `PDF_CACHE_DIR` and `PDF_CACHE_TTL` and prepare the directory.
    ///
    /// *Unset* `PDF_CACHE_DIR` means the default location; *set but empty* means
    /// the cache is off. A directory that cannot be created warns and disables
    /// the cache.
    pub fn from_env() -> Self {
        let ttl = parse_cache_ttl(&std::env::var("PDF_CACHE_TTL").unwrap_or_default());

        let Some(dir) = cache_dir_from_env() else {
            tracing::info!("PDF cache disabled (PDF_CACHE_DIR is empty)");
            return Self::disabled();
        };

        if let Err(e) = fs::create_dir_all(&dir) {
            tracing::warn!(
                dir = %dir.display(),
                error = %e,
                "cannot create the PDF cache directory, continuing without a cache"
            );
            return Self::disabled();
        }

        tracing::info!(dir = %dir.display(), ttl_secs = ttl.as_secs(), "PDF cache enabled");
        Self::new(dir, ttl)
    }

    pub fn new(dir: PathBuf, ttl: Duration) -> Self {
        Self {
            dir: Some(dir),
            ttl,
        }
    }

    pub fn disabled() -> Self {
        Self {
            dir: None,
            ttl: Duration::ZERO,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.dir.is_some()
    }

    fn path_for(&self, url: &str) -> Option<PathBuf> {
        Some(self.dir.as_ref()?.join(cache_file_name(url)))
    }

    /// Read the cached copy of `url`, whether or not it is still fresh. A
    /// missing or unreadable file is a miss.
    pub fn get(&self, url: &str) -> Option<CachedPdf> {
        let path = self.path_for(url)?;

        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::debug!(path = %path.display(), error = %e, "PDF cache miss");
                return None;
            }
        };

        // A clock that jumped backwards makes `duration_since` fail; age zero
        // then keeps a usable file usable.
        let age = fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .unwrap_or(Duration::ZERO);

        Some(CachedPdf {
            bytes: Bytes::from(bytes),
            fresh: age < self.ttl,
            age,
        })
    }

    /// Store `bytes` as the cached copy of `url`.
    ///
    /// Writes to a sibling temp file and renames it into place, so a crash
    /// cannot leave a half-written PDF behind. Failures warn and return.
    pub fn put(&self, url: &str, bytes: &[u8]) {
        let Some(path) = self.path_for(url) else {
            return;
        };

        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));

        if let Err(e) = fs::write(&tmp, bytes) {
            tracing::warn!(path = %tmp.display(), error = %e, "could not write the PDF cache entry");
            return;
        }

        if let Err(e) = fs::rename(&tmp, &path) {
            tracing::warn!(path = %path.display(), error = %e, "could not commit the PDF cache entry");
            let _ = fs::remove_file(&tmp);
            return;
        }

        tracing::debug!(path = %path.display(), bytes = bytes.len(), "wrote PDF cache entry");
    }
}

/// Filename for a plan URL: `{sha256(url)[..16 hex]}-{URL's own file name}`.
///
/// The hash makes the name unique and filesystem-safe for any URL; the readable
/// tail says which plan a file holds. SHA-256 rather than `DefaultHasher`, whose
/// output is not stable across Rust releases.
fn cache_file_name(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    let mut name = String::with_capacity(96);
    for byte in &digest[..8] {
        let _ = write!(name, "{byte:02x}");
    }

    let path = url.split_once(['?', '#']).map_or(url, |(before, _)| before);
    let tail: String = path
        .rsplit_once('/')
        .map_or(path, |(_, file_name)| file_name)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(64)
        .collect();

    if !tail.is_empty() {
        name.push('-');
        name.push_str(&tail);
    }

    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_is_stable_and_readable() {
        let url = "https://example.org/a/b/Abfuhrplan_2026.pdf";
        let name = cache_file_name(url);

        assert_eq!(name, cache_file_name(url), "must be deterministic");
        assert!(name.ends_with("-Abfuhrplan_2026.pdf"), "{name}");
        assert_eq!(name.len(), 16 + 1 + "Abfuhrplan_2026.pdf".len());
    }

    #[test]
    fn different_urls_get_different_names() {
        // Same file name, different host: only the hash prefix separates them.
        let a = cache_file_name("https://a.example/plan.pdf");
        let b = cache_file_name("https://b.example/plan.pdf");
        assert_ne!(a, b);
        assert!(a.ends_with("-plan.pdf") && b.ends_with("-plan.pdf"));
    }

    #[test]
    fn query_string_is_not_part_of_the_readable_tail() {
        let name = cache_file_name("https://example.org/plan.pdf?v=2");
        assert!(name.ends_with("-plan.pdf"), "{name}");
        // The hash covers the whole URL, so it still keys separately.
        assert_ne!(name, cache_file_name("https://example.org/plan.pdf"));
    }

    #[test]
    fn path_traversal_in_the_url_cannot_escape_the_directory() {
        let name = cache_file_name("https://example.org/..%2f..%2fetc/passwd");
        assert!(!name.contains('/'), "{name}");
        assert!(!name.contains(std::path::MAIN_SEPARATOR), "{name}");
    }

    #[test]
    fn a_disabled_cache_neither_reads_nor_writes() {
        let cache = PdfCache::disabled();
        assert!(!cache.is_enabled());
        assert!(cache.path_for("https://example.org/plan.pdf").is_none());

        cache.put("https://example.org/plan.pdf", b"%PDF-1.4");
        assert!(cache.get("https://example.org/plan.pdf").is_none());
    }
}
