use std::{
  path::{Path, PathBuf},
  process::{Command, Stdio},
  sync::{Arc, Mutex},
};

use git2::{Repository, Signature};
use thiserror::Error;
use tracing::{debug, instrument};

#[derive(Debug, Error)]
pub enum GitError {
  #[error("Failed to open git repository at {path:?}: {source}")]
  Open { path: PathBuf, source: git2::Error },

  #[error(
    "Repository at {path:?} is a bare clone; a working directory is required"
  )]
  BareRepo { path: PathBuf },

  #[error("Failed to read {path:?}: {source}")]
  ReadFile {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to write {path:?}: {source}")]
  WriteFile {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to create directory {path:?}: {source}")]
  CreateDir {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Failed to read directory {path:?}: {source}")]
  ReadDir {
    path: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("git library error: {0}")]
  Git(#[from] git2::Error),

  #[error("Failed to spawn git subprocess: {0}")]
  ProcessSpawn(#[source] std::io::Error),

  #[error("git push to remote {remote:?} failed")]
  PushFailed { remote: String },

  #[error("git pull from remote {remote:?} failed")]
  PullFailed { remote: String },

  #[error("Internal error: git mutex poisoned")]
  MutexPoisoned,
}

/// User whose edit is attributed via a `Co-authored-by:` commit trailer.
///
/// Populated from the OIDC identity claims.
#[derive(Debug, Clone)]
pub struct CommitAuthor {
  pub name: String,
  pub email: String,
}

/// Commit message as supplied by the user in the web editor.
#[derive(Debug, Clone)]
pub struct CommitMessage {
  /// Subject line (required).
  pub subject: String,
  /// Extended description (optional; may be empty).
  pub body: Option<String>,
}

/// A handle to the wiki's content git repository.
///
/// Cheap to clone — all clones share the same underlying `Repository`
/// behind a `Mutex`.  Read operations (file reads, directory walks) bypass
/// the mutex and access the working tree directly.  Write operations
/// (staging + committing) hold the mutex for the minimum duration required.
///
/// **All methods are blocking.**  Wrap with `tokio::task::spawn_blocking`
/// from async Axum handlers.
#[derive(Clone)]
pub struct WikiRepo {
  /// Absolute path to the git working tree (= content root in phase 1).
  root: PathBuf,
  /// Remote name to push to after commits (e.g. `"origin"`).
  /// `None` disables push entirely.
  remote: Option<String>,
  inner: Arc<Mutex<Repository>>,
}

impl WikiRepo {
  /// Open an existing (non-bare) git repository at `root`.
  pub fn open(root: &Path, remote: Option<String>) -> Result<Self, GitError> {
    let repo = Repository::open(root).map_err(|source| GitError::Open {
      path: root.to_owned(),
      source,
    })?;

    if repo.is_bare() {
      return Err(GitError::BareRepo {
        path: root.to_owned(),
      });
    }

    Ok(Self {
      root: root.to_owned(),
      remote,
      inner: Arc::new(Mutex::new(repo)),
    })
  }

  pub fn root(&self) -> &Path {
    &self.root
  }

  /// Read an org-mode page from the working tree.
  ///
  /// `rel_path` is relative to the repo root (e.g. `"guides/setup.org"`).
  pub fn read_page(&self, rel_path: &Path) -> Result<String, GitError> {
    let abs = self.root.join(rel_path);
    std::fs::read_to_string(&abs)
      .map_err(|source| GitError::ReadFile { path: abs, source })
  }

  /// Return relative paths of all `.org` files in the working tree, sorted.
  ///
  /// Hidden directories (names starting with `.`) are skipped, which
  /// excludes `.git` automatically.
  pub fn list_pages(&self) -> Result<Vec<PathBuf>, GitError> {
    let mut pages = Vec::new();
    collect_org_files(&self.root, &self.root, &mut pages)?;
    pages.sort();
    Ok(pages)
  }

  /// Write `content` to `rel_path`, stage it, and create a commit.
  ///
  /// - The git `Author` and `Committer` are set to `server_name`/`server_email`.
  /// - If `co_author` is supplied, a `Co-authored-by:` trailer is appended,
  ///   attributing the human who made the edit (as on GitHub/Gitea web editors).
  ///
  /// Returns the OID of the new commit.
  #[instrument(skip(self, content), fields(path = ?rel_path))]
  pub fn write_and_commit(
    &self,
    rel_path: &Path,
    content: &str,
    message: &CommitMessage,
    server_name: &str,
    server_email: &str,
    co_author: Option<&CommitAuthor>,
  ) -> Result<git2::Oid, GitError> {
    let abs = self.root.join(rel_path);

    // 1. Write file to the working tree.
    if let Some(parent) = abs.parent() {
      std::fs::create_dir_all(parent).map_err(|source| {
        GitError::CreateDir {
          path: parent.to_owned(),
          source,
        }
      })?;
    }
    std::fs::write(&abs, content)
      .map_err(|source| GitError::WriteFile { path: abs, source })?;

    // 2. Stage the file and create a commit (mutex held for minimum duration).
    let repo = self.inner.lock().map_err(|_| GitError::MutexPoisoned)?;

    let mut index = repo.index()?;
    index.add_path(rel_path)?;
    index.write()?;

    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;

    let sig = Signature::now(server_name, server_email)?;
    let full_message = build_message(message, co_author);

    // On an unborn branch (freshly initialised repo) there is no parent commit.
    let maybe_parent = match repo.head() {
      Ok(head) => Some(head.peel_to_commit()?),
      Err(e) if e.code() == git2::ErrorCode::UnbornBranch => None,
      Err(e) => return Err(e.into()),
    };
    let parents: Vec<&git2::Commit> = maybe_parent.iter().collect();

    let oid =
      repo.commit(Some("HEAD"), &sig, &sig, &full_message, &tree, &parents)?;

    debug!(%oid, "committed");
    Ok(oid)
  }

  /// Push to the configured remote.  No-op when no remote is configured.
  ///
  /// **Blocking** — shells out to `git push`.
  #[instrument(skip(self))]
  pub fn push(&self) -> Result<(), GitError> {
    let Some(remote) = &self.remote else {
      return Ok(());
    };

    // Push HEAD explicitly so the current branch is pushed by name even when
    // no upstream tracking ref has been configured yet (e.g. first push after
    // a local git-init fallback).
    let status = Command::new("git")
      .args(["push", remote, "HEAD"])
      .current_dir(&self.root)
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .status()
      .map_err(GitError::ProcessSpawn)?;

    if status.success() {
      Ok(())
    } else {
      Err(GitError::PushFailed {
        remote: remote.clone(),
      })
    }
  }

  /// Pull from the configured remote using `--ff-only`.
  /// No-op when no remote is configured.
  ///
  /// Used by the webhook handler to incorporate external commits.
  /// **Blocking** — shells out to `git pull`.
  #[instrument(skip(self))]
  pub fn pull(&self) -> Result<(), GitError> {
    let Some(remote) = &self.remote else {
      return Ok(());
    };

    let status = Command::new("git")
      .args(["pull", "--ff-only", remote])
      .current_dir(&self.root)
      .stdout(Stdio::null())
      .stderr(Stdio::null())
      .status()
      .map_err(GitError::ProcessSpawn)?;

    if status.success() {
      Ok(())
    } else {
      Err(GitError::PullFailed {
        remote: remote.clone(),
      })
    }
  }
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn build_message(
  message: &CommitMessage,
  co_author: Option<&CommitAuthor>,
) -> String {
  let mut s = message.subject.clone();

  if let Some(body) = &message.body {
    let body = body.trim();
    if !body.is_empty() {
      s.push_str("\n\n");
      s.push_str(body);
    }
  }

  if let Some(author) = co_author {
    // Git requires trailers to be separated from the body by a blank line.
    s.push_str("\n\nCo-authored-by: ");
    s.push_str(&author.name);
    s.push_str(" <");
    s.push_str(&author.email);
    s.push('>');
  }

  s
}

fn collect_org_files(
  root: &Path,
  dir: &Path,
  out: &mut Vec<PathBuf>,
) -> Result<(), GitError> {
  let entries = std::fs::read_dir(dir).map_err(|source| GitError::ReadDir {
    path: dir.to_owned(),
    source,
  })?;

  for entry in entries {
    let entry = entry.map_err(|source| GitError::ReadDir {
      path: dir.to_owned(),
      source,
    })?;
    let path = entry.path();

    // Skip hidden entries (.git, .gitignore, etc.).
    if path
      .file_name()
      .map(|n| n.to_string_lossy().starts_with('.'))
      .unwrap_or(false)
    {
      continue;
    }

    if path.is_dir() {
      collect_org_files(root, &path, out)?;
    } else if path.extension().map_or(false, |e| e == "org") {
      if let Ok(rel) = path.strip_prefix(root) {
        out.push(rel.to_owned());
      }
    }
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::process::Command as StdCommand;
  use tempfile::TempDir;

  /// Create a minimal git repo in a temp dir and return (TempDir, WikiRepo).
  fn make_repo() -> (TempDir, WikiRepo) {
    let dir = TempDir::new().unwrap();
    let path = dir.path();

    // Init with a seed file so HEAD points to a real commit.
    StdCommand::new("git")
      .args(["init", "-b", "main"])
      .current_dir(path)
      .output()
      .unwrap();
    StdCommand::new("git")
      .args(["config", "user.email", "test@example.com"])
      .current_dir(path)
      .output()
      .unwrap();
    StdCommand::new("git")
      .args(["config", "user.name", "Test"])
      .current_dir(path)
      .output()
      .unwrap();

    std::fs::write(path.join("README.org"), "#+title: Wiki\n").unwrap();

    StdCommand::new("git")
      .args(["add", "."])
      .current_dir(path)
      .output()
      .unwrap();
    StdCommand::new("git")
      .args(["commit", "-m", "init"])
      .current_dir(path)
      .output()
      .unwrap();

    let repo = WikiRepo::open(path, None).unwrap();
    (dir, repo)
  }

  #[test]
  fn read_page_roundtrip() {
    let (_dir, repo) = make_repo();
    let content = repo.read_page(Path::new("README.org")).unwrap();
    assert!(content.contains("#+title: Wiki"));
  }

  #[test]
  fn list_pages_finds_org_files() {
    let (_dir, repo) = make_repo();
    let pages = repo.list_pages().unwrap();
    assert!(pages.iter().any(|p| p == Path::new("README.org")));
  }

  #[test]
  fn write_and_commit_creates_file() {
    let (_dir, repo) = make_repo();
    let msg = CommitMessage {
      subject: "Add index".to_owned(),
      body: None,
    };
    let co = CommitAuthor {
      name: "Alice".to_owned(),
      email: "alice@example.com".to_owned(),
    };

    repo
      .write_and_commit(
        Path::new("index.org"),
        "#+title: Index\n",
        &msg,
        "Wiki Server",
        "wiki@example.com",
        Some(&co),
      )
      .unwrap();

    let content = repo.read_page(Path::new("index.org")).unwrap();
    assert!(content.contains("#+title: Index"));
  }

  #[test]
  fn commit_message_includes_co_author_trailer() {
    let co = CommitAuthor {
      name: "Bob".to_owned(),
      email: "bob@example.com".to_owned(),
    };
    let msg = CommitMessage {
      subject: "Fix typo".to_owned(),
      body: Some("Details here.".to_owned()),
    };
    let result = build_message(&msg, Some(&co));
    assert!(result.contains("Co-authored-by: Bob <bob@example.com>"));
    assert!(result.starts_with("Fix typo"));
  }
}
