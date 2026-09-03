//! The CLAUDE.md chain, in load order, with `@imports` followed.
//!
//! Depth matters in practice: the global file on this machine imports RTK.md,
//! so a reader that stopped at the roots would report the wrong instructions.

use serde::Serialize;

/// How deep imports are followed. Well past anything real, and a stop that
/// cannot be reached by accident.
const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Doc {
    /// "user", "project", or "import" — what put this file in the chain.
    pub source: String,
    pub path: String,
    pub present: bool,
    pub lines: usize,
    pub imports: Vec<Doc>,
}

/// An `@path` import: the whole line, leading whitespace aside, must be the
/// reference. Anything else is prose that happens to contain an at-sign.
fn import_of(line: &str) -> Option<&str> {
    let t = line.trim();
    let rest = t.strip_prefix('@')?;
    if rest.is_empty() || rest.contains(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => ".".to_string(),
    }
}

fn resolve_import(from: &str, target: &str) -> String {
    if target.starts_with('/') {
        return target.to_string();
    }
    format!("{}/{}", dir_of(from), target)
}

fn walk(
    source: &str,
    path: &str,
    read: &mut dyn FnMut(&str) -> Option<String>,
    seen: &mut Vec<String>,
    depth: usize,
) -> Doc {
    let text = read(path);
    let present = text.is_some();
    let body = text.unwrap_or_default();
    let lines = if present { body.lines().count() } else { 0 };

    let mut imports = Vec::new();
    if present && depth < MAX_DEPTH {
        for line in body.lines() {
            let Some(target) = import_of(line) else {
                continue;
            };
            let full = resolve_import(path, target);
            if seen.contains(&full) {
                continue; // a cycle, or the same file pulled in twice
            }
            seen.push(full.clone());
            imports.push(walk("import", &full, read, seen, depth + 1));
        }
    }

    Doc {
        source: source.to_string(),
        path: path.to_string(),
        present,
        lines,
        imports,
    }
}

/// `roots` is (source label, path), in load order.
pub fn chain(roots: &[(String, String)], read: &mut dyn FnMut(&str) -> Option<String>) -> Vec<Doc> {
    let mut seen: Vec<String> = roots.iter().map(|(_, p)| p.clone()).collect();
    roots
        .iter()
        .map(|(src, p)| walk(src, p, read, &mut seen, 0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn reader(files: Vec<(&str, &str)>) -> impl FnMut(&str) -> Option<String> {
        let map: HashMap<String, String> = files
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |p: &str| map.get(p).cloned()
    }

    #[test]
    fn a_document_reports_its_length() {
        let mut r = reader(vec![("/h/CLAUDE.md", "one\ntwo\nthree")]);
        let docs = chain(&[("user".into(), "/h/CLAUDE.md".into())], &mut r);
        assert!(docs[0].present);
        assert_eq!(docs[0].lines, 3);
    }

    #[test]
    fn an_import_is_followed_and_nested_under_the_file_that_pulled_it() {
        // The real case here: the global CLAUDE.md imports RTK.md, so a reader
        // that stopped at depth 1 would show the wrong instructions.
        let mut r = reader(vec![
            ("/h/CLAUDE.md", "@RTK.md\nrules"),
            ("/h/RTK.md", "rtk"),
        ]);
        let docs = chain(&[("user".into(), "/h/CLAUDE.md".into())], &mut r);
        assert_eq!(docs[0].imports.len(), 1);
        assert_eq!(docs[0].imports[0].path, "/h/RTK.md");
        assert!(docs[0].imports[0].present);
    }

    #[test]
    fn an_import_that_is_not_there_is_shown_as_missing_not_skipped() {
        let mut r = reader(vec![("/h/CLAUDE.md", "@gone.md")]);
        let docs = chain(&[("user".into(), "/h/CLAUDE.md".into())], &mut r);
        assert_eq!(docs[0].imports.len(), 1);
        assert!(!docs[0].imports[0].present);
    }

    #[test]
    fn a_cycle_terminates() {
        let mut r = reader(vec![("/h/a.md", "@b.md"), ("/h/b.md", "@a.md")]);
        let docs = chain(&[("user".into(), "/h/a.md".into())], &mut r);
        // b is reached once; a is not re-entered.
        assert_eq!(docs[0].imports[0].path, "/h/b.md");
        assert!(docs[0].imports[0].imports.is_empty());
    }

    #[test]
    fn an_absent_root_is_present_in_the_list_and_marked_absent() {
        // "./CLAUDE.md — not present" is the useful answer for a project.
        let mut r = reader(vec![]);
        let docs = chain(&[("project".into(), "/p/CLAUDE.md".into())], &mut r);
        assert_eq!(docs.len(), 1);
        assert!(!docs[0].present);
        assert_eq!(docs[0].lines, 0);
    }

    #[test]
    fn an_at_sign_mid_sentence_is_not_an_import() {
        let mut r = reader(vec![("/h/CLAUDE.md", "email me @ matt@example.com")]);
        let docs = chain(&[("user".into(), "/h/CLAUDE.md".into())], &mut r);
        assert!(docs[0].imports.is_empty());
    }
}
