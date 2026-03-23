use std::path::PathBuf;
use thiserror::Error;
use tracing::{debug, instrument, warn};

#[derive(Debug, Error)]
pub enum CacheError {
  #[error("Failed to read cache entry at {path:?}: {source}")]
  Read {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to write cache entry at {path:?}: {source}")]
  Write {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to create cache directory {path:?}: {source}")]
  CreateDir {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },
}

/// Filesystem-backed HTML fragment cache.
///
/// Each wiki page maps to a `.html` file under `dir`, mirroring the
/// page's directory structure.  For example, `guides/setup.org` caches
/// to `<dir>/guides/setup.html`.
///
/// When constructed with `Cache::disabled()` all operations are no-ops,
/// which is useful for development or low-traffic deployments that prefer
/// to render on every request.
#[derive(Debug, Clone)]
pub struct Cache {
  dir: Option<PathBuf>,
}

impl Cache {
  /// Create a cache backed by `dir`.  The directory is created on first write.
  pub fn new(dir: PathBuf) -> Self {
    Self { dir: Some(dir) }
  }

  /// Create a no-op cache.  All reads return `None`; writes are silently dropped.
  pub fn disabled() -> Self {
    Self { dir: None }
  }

  /// Build the filesystem path for a page's cached HTML.
  fn entry_path(&self, page_key: &str) -> Option<PathBuf> {
    self.dir.as_ref().map(|d| {
      let html_key = page_key
        .strip_suffix(".org")
        .map(|stem| format!("{stem}.html"))
        .unwrap_or_else(|| format!("{page_key}.html"));
      d.join(html_key)
    })
  }

  /// Return a cached HTML fragment for `page_key`, or `None` on a miss.
  ///
  /// `page_key` is the page's path relative to the content root
  /// (e.g. `"guides/setup.org"`).
  pub fn get(&self, page_key: &str) -> Result<Option<String>, CacheError> {
    let Some(path) = self.entry_path(page_key) else {
      return Ok(None);
    };

    match std::fs::read_to_string(&path) {
      Ok(html) => {
        debug!(page_key, "cache hit");
        Ok(Some(html))
      }
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
      Err(source) => Err(CacheError::Read { path, source }),
    }
  }

  /// Store an HTML fragment in the cache.
  #[instrument(skip(self, html), fields(bytes = html.len()))]
  pub fn set(&self, page_key: &str, html: &str) -> Result<(), CacheError> {
    let Some(path) = self.entry_path(page_key) else {
      return Ok(());
    };

    if let Some(parent) = path.parent() {
      std::fs::create_dir_all(parent).map_err(|source| {
        CacheError::CreateDir {
          path: parent.to_owned(),
          source,
        }
      })?;
    }

    std::fs::write(&path, html)
      .map_err(|source| CacheError::Write { path, source })
  }

  /// Remove the cached entry for `page_key`.
  ///
  /// A missing entry is not an error — this is idempotent.
  pub fn invalidate(&self, page_key: &str) {
    let Some(path) = self.entry_path(page_key) else {
      return;
    };

    if let Err(e) = std::fs::remove_file(&path) {
      if e.kind() != std::io::ErrorKind::NotFound {
        warn!(?path, "failed to invalidate cache entry: {e}");
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use tempfile::TempDir;

  fn temp_cache() -> (TempDir, Cache) {
    let dir = TempDir::new().unwrap();
    let cache = Cache::new(dir.path().to_owned());
    (dir, cache)
  }

  #[test]
  fn miss_on_empty_cache() {
    let (_dir, cache) = temp_cache();
    assert!(cache.get("index.org").unwrap().is_none());
  }

  #[test]
  fn set_then_get() {
    let (_dir, cache) = temp_cache();
    cache.set("index.org", "<p>hello</p>").unwrap();
    assert_eq!(
      cache.get("index.org").unwrap().as_deref(),
      Some("<p>hello</p>")
    );
  }

  #[test]
  fn invalidate_removes_entry() {
    let (_dir, cache) = temp_cache();
    cache.set("index.org", "<p>hello</p>").unwrap();
    cache.invalidate("index.org");
    assert!(cache.get("index.org").unwrap().is_none());
  }

  #[test]
  fn nested_path_is_mirrored() {
    let (_dir, cache) = temp_cache();
    cache.set("guides/setup.org", "<p>setup</p>").unwrap();
    assert_eq!(
      cache.get("guides/setup.org").unwrap().as_deref(),
      Some("<p>setup</p>")
    );
  }

  #[test]
  fn disabled_cache_always_misses() {
    let cache = Cache::disabled();
    cache.set("index.org", "<p>hello</p>").unwrap();
    assert!(cache.get("index.org").unwrap().is_none());
  }
}
