use std::{
  io::Write as _,
  path::{Path, PathBuf},
  process::{Command, Stdio},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExportError {
  #[error("Failed to spawn Pandoc at {pandoc_bin:?}: {source}")]
  Spawn {
    pandoc_bin: PathBuf,
    #[source]
    source: std::io::Error,
  },

  #[error("Pandoc exited with status {status}:\n{stderr}")]
  PandocFailed { status: i32, stderr: String },

  #[error("Pandoc produced non-UTF-8 output: {0}")]
  Utf8(#[from] std::string::FromUtf8Error),

  #[error("I/O error communicating with Pandoc: {0}")]
  Io(#[from] std::io::Error),
}

/// Convert org-mode source text to an HTML body fragment.
///
/// Shells out to Pandoc (`--from org --to html5`, no `--standalone`).
/// The result is suitable for embedding inside a page template — it does
/// not include a `<html>`/`<head>`/`<body>` wrapper.
///
/// **Blocking.** Call from async contexts via `tokio::task::spawn_blocking`.
#[tracing::instrument(skip(org_content), fields(bytes = org_content.len()))]
pub fn export_to_html(
  org_content: &str,
  pandoc_bin: &Path,
) -> Result<String, ExportError> {
  let mut child = Command::new(pandoc_bin)
    .args(["--from", "org", "--to", "html5"])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|source| ExportError::Spawn {
      pandoc_bin: pandoc_bin.to_owned(),
      source,
    })?;

  // Write content to stdin then close the pipe to signal EOF.
  if let Some(mut stdin) = child.stdin.take() {
    stdin.write_all(org_content.as_bytes())?;
    // stdin drops here, closing the pipe.
  }

  let output = child.wait_with_output()?;

  if !output.status.success() {
    return Err(ExportError::PandocFailed {
      status: output.status.code().unwrap_or(-1),
      stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    });
  }

  Ok(String::from_utf8(output.stdout)?)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::path::Path;

  /// Smoke-test that a minimal org document produces non-empty HTML.
  /// Requires `pandoc` on PATH (present in the Nix dev shell).
  #[test]
  fn roundtrip_minimal() {
    let html =
      export_to_html("#+title: Test\n\nHello, world!\n", Path::new("pandoc"))
        .unwrap();
    assert!(html.contains("Hello, world!"), "unexpected output: {html}");
  }
}
