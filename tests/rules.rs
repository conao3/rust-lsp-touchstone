//! docs/rules.md lists every finding code used in src/, and nothing else.

use std::collections::BTreeSet;
use std::fs;

const AREAS: &[&str] = &[
    "framing",
    "jsonrpc",
    "lifecycle",
    "types",
    "capability",
    "sync",
    "positions",
    "cancel",
    "progress",
    "client",
];

/// The `<area>/<name>` codes in `text` that start right after `quote` and end at the next one.
fn codes(text: &str, quote: char) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for area in AREAS {
        let opening = format!("{quote}{area}/");
        for (start, _) in text.match_indices(&opening) {
            let rest = &text[start + 1..];
            let Some(end) = rest.find(quote) else {
                continue;
            };
            let name = &rest[area.len() + 1..end];
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                found.insert(rest[..end].to_string());
            }
        }
    }
    found
}

#[test]
fn every_code_is_documented() {
    let mut used = BTreeSet::new();
    for entry in fs::read_dir("src").unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            used.extend(codes(&fs::read_to_string(path).unwrap(), '"'));
        }
    }
    let documented = codes(&fs::read_to_string("docs/rules.md").unwrap(), '`');
    let missing: Vec<_> = used.difference(&documented).collect();
    let stale: Vec<_> = documented.difference(&used).collect();
    assert!(
        missing.is_empty(),
        "codes missing from docs/rules.md: {missing:?}"
    );
    assert!(
        stale.is_empty(),
        "codes in docs/rules.md that src/ does not use: {stale:?}"
    );
}
