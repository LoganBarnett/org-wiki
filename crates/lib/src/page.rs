use serde::Serialize;

/// Metadata extracted from an org-mode document's keyword preamble.
///
/// Currently covers only the document title.  Additional fields (author,
/// date, filetags) will be added via `orgize` as the need arises; for now
/// we avoid depending on the unstable 0.10-alpha API for production paths.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PageMeta {
  /// Value of `#+title:` (case-insensitive), if present in the preamble.
  pub title: Option<String>,
}

impl PageMeta {
  /// Parse document metadata from org-mode source text.
  ///
  /// Scans lines before the first headline (`*`-prefixed line) for
  /// known keywords.  Pandoc handles the actual content export; this
  /// function only extracts the fields the server needs for navigation
  /// and templating.
  pub fn parse(content: &str) -> Self {
    let title = content
      .lines()
      .take_while(|line| !line.starts_with('*'))
      .find_map(|line| {
        // "#+title:" is 8 bytes.
        if line.len() >= 8 && line[..8].eq_ignore_ascii_case("#+title:") {
          let v = line[8..].trim();
          if v.is_empty() {
            None
          } else {
            Some(v.to_owned())
          }
        } else {
          None
        }
      });

    Self { title }
  }

  /// Return the title if present, otherwise a fallback derived from `file_stem`.
  pub fn display_title<'a>(&'a self, file_stem: &'a str) -> &'a str {
    self.title.as_deref().unwrap_or(file_stem)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn extracts_title() {
    let meta = PageMeta::parse("#+title: My Page\n\n* Section\n");
    assert_eq!(meta.title.as_deref(), Some("My Page"));
  }

  #[test]
  fn title_is_case_insensitive() {
    let meta = PageMeta::parse("#+TITLE: Upper\n");
    assert_eq!(meta.title.as_deref(), Some("Upper"));
  }

  #[test]
  fn title_stops_at_headline() {
    // A #+title: after a headline is not a document keyword.
    let meta = PageMeta::parse("* Headline\n#+title: Not a title\n");
    assert!(meta.title.is_none());
  }

  #[test]
  fn missing_title_returns_none() {
    let meta = PageMeta::parse("#+author: Someone\n\n* Section\n");
    assert!(meta.title.is_none());
  }
}
