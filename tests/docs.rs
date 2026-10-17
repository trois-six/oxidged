//! `docs/` is a conformant OKF v0.2 bundle
//! (<https://github.com/GoogleCloudPlatform/open-knowledge-format/blob/main/SPEC.md>):
//! every document has its frontmatter, the index lists each once with its
//! description, and every relative link of the documentation resolves.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The documents of `docs/` but the index, by file name.
fn documents() -> BTreeMap<String, String> {
    std::fs::read_dir(root().join("docs"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .filter(|p| p.file_name().is_some_and(|n| n != "index.md"))
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(&p).unwrap())
        })
        .collect()
}

/// The frontmatter lines of a document and its body.
fn split(name: &str, text: &str) -> (Vec<String>, String) {
    let rest = text
        .strip_prefix("---\n")
        .unwrap_or_else(|| panic!("{name}: no frontmatter"));
    let (front, body) = rest
        .split_once("\n---\n")
        .unwrap_or_else(|| panic!("{name}: unterminated frontmatter"));
    (
        front.lines().map(str::to_string).collect(),
        body.to_string(),
    )
}

/// The value of a top-level frontmatter key, its quotes removed.
fn value<'a>(front: &'a [String], key: &str) -> Option<&'a str> {
    front.iter().find_map(|l| {
        let v = l.strip_prefix(key)?.strip_prefix(':')?.trim();
        Some(
            v.strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(v),
        )
    })
}

fn is_utc_datetime(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 20
        && b.iter().enumerate().all(|(i, &c)| match i {
            4 | 7 => c == b'-',
            10 => c == b'T',
            13 | 16 => c == b':',
            19 => c == b'Z',
            _ => c.is_ascii_digit(),
        })
}

#[test]
fn every_document_has_its_frontmatter() {
    let docs = documents();
    assert!(!docs.is_empty());
    for (name, text) in &docs {
        let (front, body) = split(name, text);
        let ty = value(&front, "type").unwrap_or_default();
        assert!(!ty.is_empty(), "{name}: no type");
        let title = value(&front, "title").unwrap_or_else(|| panic!("{name}: no title"));
        let h1 = body
            .lines()
            .find_map(|l| l.strip_prefix("# "))
            .unwrap_or_else(|| panic!("{name}: no H1"));
        assert_eq!(title, h1, "{name}: the title is the H1");
        let description = value(&front, "description").unwrap_or_default();
        assert!(
            description.ends_with('.') && !description.contains('\n'),
            "{name}: one sentence of description"
        );
        let tags = value(&front, "tags").unwrap_or_default();
        assert!(
            tags.starts_with("[oxidged") && tags.ends_with(']'),
            "{name}: tags start with oxidged"
        );
        let generated = value(&front, "generated").unwrap_or_default();
        let at = generated
            .strip_prefix("{ by: human:maintainer, at: ")
            .and_then(|g| g.strip_suffix(" }"))
            .unwrap_or_else(|| panic!("{name}: generated: {generated}"));
        assert!(is_utc_datetime(at), "{name}: generated.at {at}");
        assert!(!front.iter().any(|l| l.starts_with("timestamp:")), "{name}");
        assert!(!body.contains("\n# Citations"), "{name}");
    }
}

#[test]
fn the_index_lists_every_document_once() {
    let docs = documents();
    let index = std::fs::read_to_string(root().join("docs/index.md")).unwrap();
    let rest = index
        .strip_prefix("---\nokf_version: \"0.2\"\n---\n")
        .expect("the index's frontmatter is only okf_version");
    let mut listed = BTreeMap::new();
    for line in rest.lines().filter(|l| !l.is_empty()) {
        if line.starts_with("# ") {
            continue;
        }
        let entry = line
            .strip_prefix("* [")
            .unwrap_or_else(|| panic!("index: {line}"));
        let (title, rest) = entry.split_once("](").unwrap();
        let (file, description) = rest.split_once(") - ").unwrap();
        assert!(
            listed
                .insert(file.to_string(), (title, description))
                .is_none(),
            "{file} listed twice"
        );
    }
    let names: Vec<_> = docs.keys().cloned().collect();
    let entries: Vec<_> = listed.keys().cloned().collect();
    assert_eq!(names, entries, "the index lists every document");
    for (name, text) in &docs {
        let (front, _) = split(name, text);
        let (title, description) = listed[name];
        assert_eq!(Some(title), value(&front, "title"), "{name}");
        assert_eq!(Some(description), value(&front, "description"), "{name}");
    }
}

/// The anchors of a markdown document: its headings as GitHub slugs them,
/// and its explicit `<a id>`s.
fn anchors(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.starts_with("```") {
            fenced = !fenced;
        }
        if fenced {
            continue;
        }
        if let Some(h) = line.trim_start_matches('#').strip_prefix(' ')
            && line.starts_with('#')
        {
            let slug: String = h
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
                .map(|c| if c == ' ' { '-' } else { c })
                .collect();
            out.push(slug);
        }
        if let Some(id) = line
            .strip_prefix("<a id=\"")
            .and_then(|l| l.split_once('"'))
        {
            out.push(id.0.to_string());
        }
    }
    out
}

/// The targets of the inline links of a markdown text, outside code.
fn links(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.starts_with("```") {
            fenced = !fenced;
        }
        if fenced {
            continue;
        }
        // Inline code holds no link.
        let prose: String = line
            .split('`')
            .enumerate()
            .filter(|(i, _)| i % 2 == 0)
            .map(|(_, part)| part)
            .collect();
        let mut rest = prose.as_str();
        while let Some(at) = rest.find("](") {
            let after = &rest[at + 2..];
            let Some(end) = after.find(')') else { break };
            out.push(after[..end].to_string());
            rest = &after[end..];
        }
    }
    out
}

#[test]
fn relative_links_resolve() {
    let mut files: Vec<PathBuf> = documents()
        .keys()
        .map(|n| root().join("docs").join(n))
        .collect();
    files.extend(
        ["docs/index.md", "README.md", "AGENTS.md", "cli/README.md"]
            .iter()
            .map(|f| root().join(f)),
    );
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        let dir = file.parent().unwrap();
        for link in links(&text) {
            if link.contains("://") || link.starts_with("mailto:") {
                continue;
            }
            let (path, anchor) = match link.split_once('#') {
                Some((p, a)) => (p, Some(a)),
                None => (link.as_str(), None),
            };
            let target = if path.is_empty() {
                file.clone()
            } else {
                dir.join(path)
            };
            assert!(target.exists(), "{}: {link} does not exist", file.display());
            if let Some(anchor) = anchor
                && Path::new(path).extension().is_none_or(|e| e == "md")
                && target.is_file()
            {
                let found = anchors(&std::fs::read_to_string(&target).unwrap());
                assert!(
                    found.iter().any(|a| a == anchor),
                    "{}: {link}: no anchor {anchor}",
                    file.display()
                );
            }
        }
    }
}
