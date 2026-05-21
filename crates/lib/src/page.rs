use orgize::Org;
use serde::Serialize;

/// Metadata extracted from an org-mode document's keyword preamble.
///
/// All fields come from the top-level keyword section (before the first
/// headline).  Parsing goes through `orgize`'s rowan-based AST; no string
/// matching against org content lives in this file.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PageMeta {
  /// Value of `#+title:`, joined with spaces when the keyword repeats.
  pub title: Option<String>,
  /// Value of `#+author:`, joined with spaces when the keyword repeats.
  pub author: Option<String>,
  /// Value of `#+date:`, joined with spaces when the keyword repeats.
  pub date: Option<String>,
  /// Tags from `#+filetags:`.  Org's convention is colon-delimited
  /// (`:tag1:tag2:`); we accept either that form or whitespace
  /// separation, and merge tags across repeated `#+filetags:` lines.
  pub filetags: Vec<String>,
}

impl PageMeta {
  pub fn parse(content: &str) -> Self {
    let doc = Org::parse(content).document();

    let title = doc.title();

    let author = join_keyword(&doc, "AUTHOR");
    let date = join_keyword(&doc, "DATE");

    // Inner collect is the ownership shim: kw.value()'s split iterator
    // borrows from a Token that drops at the end of the closure.
    let filetags = doc
      .keywords()
      .filter(|kw| kw.key().eq_ignore_ascii_case("FILETAGS"))
      .flat_map(|kw| {
        kw.value()
          .split(|c: char| c == ':' || c.is_whitespace())
          .filter(|s| !s.is_empty())
          .map(String::from)
          .collect::<Vec<_>>()
      })
      .collect();

    Self {
      title,
      author,
      date,
      filetags,
    }
  }

  /// Return the title if present, otherwise a fallback derived from `file_stem`.
  pub fn display_title<'a>(&'a self, file_stem: &'a str) -> &'a str {
    self.title.as_deref().unwrap_or(file_stem)
  }
}

fn join_keyword(doc: &orgize::ast::Document, name: &str) -> Option<String> {
  doc
    .keywords()
    .filter(|kw| kw.key().eq_ignore_ascii_case(name))
    .fold(Option::<String>::None, |acc, cur| {
      let mut s = acc.unwrap_or_default();
      if !s.is_empty() {
        s.push(' ');
      }
      s.push_str(cur.value().trim());
      Some(s)
    })
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

  #[test]
  fn repeated_title_joins_with_space() {
    let meta = PageMeta::parse("#+TITLE: hello\n#+TITLE: world\n");
    assert_eq!(meta.title.as_deref(), Some("hello world"));
  }

  #[test]
  fn extracts_author_and_date() {
    let meta = PageMeta::parse(
      "#+title: T\n#+author: Logan Barnett\n#+date: 2026-05-21\n",
    );
    assert_eq!(meta.author.as_deref(), Some("Logan Barnett"));
    assert_eq!(meta.date.as_deref(), Some("2026-05-21"));
  }

  #[test]
  fn extracts_filetags_colon_form() {
    let meta = PageMeta::parse("#+FILETAGS: :rust:wiki:\n");
    assert_eq!(meta.filetags, vec!["rust", "wiki"]);
  }

  #[test]
  fn filetags_merge_across_lines() {
    let meta = PageMeta::parse("#+filetags: :rust:\n#+filetags: :wiki:\n");
    assert_eq!(meta.filetags, vec!["rust", "wiki"]);
  }

  #[test]
  fn missing_filetags_is_empty() {
    let meta = PageMeta::parse("#+title: t\n");
    assert!(meta.filetags.is_empty());
  }
}
