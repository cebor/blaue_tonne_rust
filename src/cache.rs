//! On-disk cache for the plan PDFs.
//!
//! Plans change once a year, so downloading them again on every process start is
//! almost always a re-download of identical bytes. Caching them buys two things:
//! a start that needs no network at all, and — through a deliberately kept
//! *stale* copy — a start that still succeeds when the source is unreachable.
//!
//! Everything here happens inside [`crate::index::build_index`], before the
//! listener binds. That is why the I/O is plain blocking `std::fs`: at that point
//! the runtime has nothing else to do, so there is no executor to starve and no
//! reason to pull in tokio's `fs` feature.
//!
//! The cache is an optimization, never a data path. Every failure in this module
//! — unwritable directory, unreadable file, failed rename — degrades to "no
//! cache" with a log line. None of it can make the service fail.

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
/// `dir: None` is a fully disabled cache, and it is the *same* code path rather
/// than a special case: `get` and `put` become no-ops. Three things produce it —
/// an empty `PDF_CACHE_DIR`, a directory that cannot be created, and
/// [`PdfCache::disabled`] in tests — and none of them needs its own branch
/// anywhere else in the crate.
pub struct PdfCache {
    dir: Option<PathBuf>,
    ttl: Duration,
}

impl PdfCache {
    /// Read `PDF_CACHE_DIR` and `PDF_CACHE_TTL` and prepare the directory.
    ///
    /// *Unset* `PDF_CACHE_DIR` means the default location; *set but empty* means
    /// the cache is off. They are different on purpose — there is no second
    /// `PDF_CACHE_ENABLED` variable that could contradict the path.
    ///
    /// A directory that cannot be created is a warning, not an error: a
    /// read-only filesystem should cost the cache, not the service.
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

    /// Read the cached copy of `url`, whether or not it is still fresh.
    ///
    /// A missing or unreadable file is a miss, not an error — the caller's next
    /// move is a download either way.
    pub fn get(&self, url: &str) -> Option<CachedPdf> {
        let path = self.path_for(url)?;

        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::debug!(path = %path.display(), error = %e, "PDF cache miss");
                return None;
            }
        };

        // A clock that jumped backwards makes `duration_since` fail. Treating
        // that as age zero keeps a usable file usable; the alternative would be
        // re-downloading every plan after every clock correction.
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
    /// Writes to a temporary file in the same directory and renames it into
    /// place, so a crash or a full disk cannot leave a half-written PDF behind
    /// that a later start would read back as corrupt.
    ///
    /// Returns `()` rather than `Result` deliberately: there is no caller that
    /// could act on a failure differently than by carrying on with the bytes it
    /// already has in memory.
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

/// Filename for a plan URL: a hash prefix plus the URL's own file name.
///
/// The hash is what makes the name unique and filesystem-safe for any URL; the
/// readable tail is what makes `ls` on the cache directory tell you which plan
/// is which. A cryptographic hash rather than `DefaultHasher`, whose output is
/// explicitly not stable across Rust releases — that would silently empty the
/// cache on every toolchain upgrade.
fn cache_file_name(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    let mut name = String::with_capacity(96);
    for byte in &digest[..8] {
        let _ = write!(name, "{byte:02x}");
    }

    let tail: String = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .next()
        .unwrap_or_default()
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
        // Same file name, different host: the hash prefix has to separate them,
        // otherwise two plans would overwrite each other's cache entry.
        let a = cache_file_name("https://a.example/plan.pdf");
        let b = cache_file_name("https://b.example/plan.pdf");
        assert_ne!(a, b);
        assert!(a.ends_with("-plan.pdf") && b.ends_with("-plan.pdf"));
    }

    #[test]
    fn query_string_is_not_part_of_the_readable_tail() {
        let name = cache_file_name("https://example.org/plan.pdf?v=2");
        assert!(name.ends_with("-plan.pdf"), "{name}");
        // …but it still keys separately, because the hash covers the whole URL.
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

        // Must not panic and must not produce a file anywhere.
        cache.put("https://example.org/plan.pdf", b"%PDF-1.4");
        assert!(cache.get("https://example.org/plan.pdf").is_none());
    }
}
