//! Source projections, deliberately limited to files and literal text matches.
//!
//! Cursors carry a query and a UTF-8 byte offset into its deterministic rendering.
//! They are read-only bookmarks, not authorization tokens or semantic indexes.
use crate::project::{normalize_path, Project, Snapshot};
use crate::Result;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

const DEFAULT_BUDGET: usize = 12_000;
const MIN_BUDGET: usize = 1_024;
const MAX_BUDGET: usize = 32_000;
const MAX_CURSOR: usize = 32_768;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Query {
    text: Option<String>,
    path: Option<String>,
    start: Option<usize>,
    end: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u8,
    snapshot: String,
    query: Query,
    offset: usize,
}

struct Options {
    query: Query,
    budget: usize,
    cursor: Option<String>,
    help: bool,
}

const HELP: &str =
    "aienv context [QUERY] [--path FILE] [--start LINE] [--end LINE] [--budget BYTES]\n\
aienv context --cursor TOKEN [--budget BYTES]\n\
No query: indexed file/hash overview. QUERY: literal, case-sensitive text matches.\n\
--path: indexed source with 1-based inclusive line range; QUERY can filter it.\n\
Budget is 1024..32000 UTF-8 bytes, including snapshot and continuation markers.\n\
Cursors resume only the same source/config snapshot. This is not an AST or call graph.\n";

pub fn run(project: &Project, args: &[String]) -> Result<String> {
    let options = parse(args)?;
    if options.help {
        return Ok(HELP.to_owned());
    }
    let snapshot = project.snapshot()?;
    execute(&snapshot, options)
}

pub fn index(project: &Project, args: &[String]) -> Result<String> {
    run(project, args)
}

fn number(value: &str, flag: &str) -> Result<usize> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{flag} requires a positive integer"))?;
    if parsed == 0 {
        return Err(format!("{flag} requires a positive integer").into());
    }
    Ok(parsed)
}

fn parse(args: &[String]) -> Result<Options> {
    let mut query = Query::default();
    let mut budget = DEFAULT_BUDGET;
    let mut cursor = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut words = Vec::new();
    let mut positional = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if !positional && arg == "--" {
            positional = true;
        } else if !positional && (arg == "--help" || arg == "-h") {
            return Ok(Options {
                query,
                budget,
                cursor,
                help: true,
            });
        } else if !positional && arg.starts_with('-') {
            if !matches!(
                arg.as_str(),
                "--path" | "--start" | "--end" | "--budget" | "--cursor"
            ) {
                return Err(format!("Unknown context option: {arg}; use context --help").into());
            }
            if !seen.insert(arg.clone()) {
                return Err(format!("Repeated context option: {arg}").into());
            }
            i += 1;
            let value = args
                .get(i)
                .ok_or_else(|| format!("Missing value for {arg}"))?;
            match arg.as_str() {
                "--path" => query.path = Some(value.clone()),
                "--start" => query.start = Some(number(value, arg)?),
                "--end" => query.end = Some(number(value, arg)?),
                "--budget" => budget = number(value, arg)?,
                "--cursor" => cursor = Some(value.clone()),
                _ => unreachable!(),
            }
        } else {
            words.push(arg.as_str());
        }
        i += 1;
    }
    if !words.is_empty() {
        query.text = Some(words.join(" "));
    }
    if !(MIN_BUDGET..=MAX_BUDGET).contains(&budget) {
        return Err("--budget must be 1024..32000 UTF-8 bytes".into());
    }
    if cursor.is_some() && query != Query::default() {
        return Err("Resume with --cursor and optional --budget only; start a fresh query to change projection".into());
    }
    validate_query(&query)?;
    Ok(Options {
        query,
        budget,
        cursor,
        help: false,
    })
}

fn validate_query(query: &Query) -> Result<()> {
    if let Some(text) = &query.text {
        if text.is_empty() || text.len() > 2_048 || text.contains(['\n', '\r', '\0']) {
            return Err(
                "QUERY must be nonempty, single-line text of at most 2048 UTF-8 bytes".into(),
            );
        }
    }
    if let Some(path) = &query.path {
        if path.len() > 4_096 || normalize_path(path)? != *path {
            return Err("--path must be a normalized relative indexed source path".into());
        }
    } else if query.start.is_some() || query.end.is_some() {
        return Err("--start and --end require --path".into());
    }
    let start = query.start.unwrap_or(1);
    if start == 0 || query.end.is_some_and(|end| end == 0 || end < start) {
        return Err("Line range must be positive and --end must be at least --start".into());
    }
    Ok(())
}

fn execute(snapshot: &Snapshot, options: Options) -> Result<String> {
    let (query, offset) = if let Some(encoded) = options.cursor {
        let cursor = decode(&encoded)?;
        if cursor.snapshot != snapshot.hash {
            return Err("Stale context cursor: indexed source or configuration changed; start a fresh query".into());
        }
        validate_query(&cursor.query)?;
        (cursor.query, cursor.offset)
    } else {
        (options.query, 0)
    };
    page(snapshot, &query, offset, options.budget)
}

fn visit(
    snapshot: &Snapshot,
    query: &Query,
    mut emit: impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    if query.path.is_none() && query.text.is_none() {
        emit("files (indexed UTF-8 text; exact hashes for edits)\n")?;
        for entry in snapshot.files.values() {
            emit(&format!(
                "{} hash={} bytes={} lines={}\n",
                entry.path,
                entry.hash,
                entry.text.len(),
                entry.text.lines().count()
            ))?;
        }
        if snapshot.files.is_empty() {
            emit("No indexed source files.\n")?;
        }
        return Ok(());
    }

    let selected = if let Some(path) = &query.path {
        vec![snapshot.files.get(path).ok_or_else(|| {
            format!(
                "Path is not indexed: {path}; inspect context/index overview for available files"
            )
        })?]
    } else {
        snapshot.files.values().collect()
    };
    if let Some(text) = &query.text {
        emit(&format!(
            "text matches (literal, case-sensitive) query={}\n",
            serde_json::to_string(text)?
        ))?;
    }
    let mut matches = 0;
    for entry in selected {
        let count = entry.text.lines().count();
        let start = query.start.unwrap_or(1);
        let end = query.end.unwrap_or(count);
        if query.path.is_some() {
            if start > count.max(1) {
                return Err(format!(
                    "--start {start} is beyond {} lines in {}",
                    count, entry.path
                )
                .into());
            }
            // End beyond EOF is an explicit range selection, reported as the actual end.
            emit(&format!(
                "{} hash={} bytes={} lines={} selected={}..{} newline={}\n",
                entry.path,
                entry.hash,
                entry.text.len(),
                count,
                start.min(count),
                end.min(count),
                if entry.text.contains("\r\n") {
                    "CRLF-or-mixed"
                } else {
                    "LF"
                }
            ))?;
            if count == 0 && query.text.is_none() {
                emit("(empty file)\n")?;
            }
        }
        for (index, line) in entry.text.lines().enumerate() {
            let line_number = index + 1;
            if line_number < start || line_number > end {
                continue;
            }
            if let Some(text) = &query.text {
                if line.contains(text) {
                    emit(&format!("{}:{}: {}\n", entry.path, line_number, line))?;
                    matches += 1;
                }
            } else {
                emit(&format!("{line_number} | {line}\n"))?;
            }
        }
    }
    if query.text.is_some() && matches == 0 {
        emit("No text matches.\n")?;
    }
    Ok(())
}

#[cfg(test)]
fn render(snapshot: &Snapshot, query: &Query) -> Result<String> {
    let mut output = String::new();
    visit(snapshot, query, |piece| {
        output.push_str(piece);
        Ok(())
    })?;
    Ok(output)
}

fn encode(cursor: &Cursor) -> Result<String> {
    let bytes = serde_json::to_vec(cursor)?;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(output, "{byte:02x}")?;
    }
    Ok(output)
}

fn decode(encoded: &str) -> Result<Cursor> {
    if encoded.is_empty()
        || encoded.len() > MAX_CURSOR
        || encoded.len() % 2 != 0
        || !encoded.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Malformed context cursor: expected bounded hex-encoded JSON".into());
    }
    let bytes: Vec<u8> = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |b: u8| {
                if b <= b'9' {
                    b - b'0'
                } else {
                    b.to_ascii_lowercase() - b'a' + 10
                }
            };
            digit(pair[0]) * 16 + digit(pair[1])
        })
        .collect();
    let cursor: Cursor =
        serde_json::from_slice(&bytes).map_err(|_| "Malformed context cursor JSON")?;
    if cursor.version != 1
        || cursor.snapshot.len() != 64
        || !cursor.snapshot.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Malformed or unsupported context cursor".into());
    }
    Ok(cursor)
}

fn footer(snapshot: &Snapshot, query: &Query, offset: usize) -> Result<String> {
    Ok(format!(
        "\ncontinue {}\n",
        encode(&Cursor {
            version: 1,
            snapshot: snapshot.hash.clone(),
            query: query.clone(),
            offset,
        })?
    ))
}

fn page(snapshot: &Snapshot, query: &Query, offset: usize, budget: usize) -> Result<String> {
    let header = format!(
        "snapshot {}\nindexed {} omitted {}\nprojection-byte-offset {offset}\n\n",
        snapshot.hash,
        snapshot.files.len(),
        snapshot.omitted
    );
    let end_marker = "\nend\n";
    let capacity = budget
        .checked_sub(header.len() + end_marker.len())
        .ok_or("Budget is too small for this context header; increase --budget")?;
    // Count the deterministic projection, but retain only one bounded page.
    // An enormous match set never becomes an enormous intermediate String.
    let mut total = 0usize;
    let mut captured = String::with_capacity(capacity);
    let mut full = false;
    visit(snapshot, query, |piece| {
        let start = total;
        total = total
            .checked_add(piece.len())
            .ok_or("Projection size overflow")?;
        if offset >= total || full {
            return Ok(());
        }
        let local = offset.saturating_sub(start);
        if !piece.is_char_boundary(local) {
            return Err("Context cursor offset splits a UTF-8 character".into());
        }
        let remainder = &piece[local..];
        let mut take = remainder.len().min(capacity - captured.len());
        while !remainder.is_char_boundary(take) {
            take -= 1;
        }
        captured.push_str(&remainder[..take]);
        full = take < remainder.len();
        Ok(())
    })?;
    if offset >= total {
        return Err("Context cursor offset is out of bounds".into());
    }
    if total - offset <= capacity {
        return Ok(format!("{header}{captured}{end_marker}"));
    }
    // usize::MAX has the widest possible decimal offset field. Reserve its
    // complete footer before selecting any source bytes for a continued page.
    let reserved = header.len() + footer(snapshot, query, usize::MAX)?.len();
    let available = budget
        .checked_sub(reserved)
        .filter(|available| *available > 3)
        .ok_or("Budget is too small for this query's continuation cursor; increase --budget")?;
    let mut end = available.min(captured.len());
    while !captured.is_char_boundary(end) {
        end -= 1;
    }
    // Prefer complete lines when that does not waste most of the page. Long lines
    // remain fully accessible through byte offsets; no line-length truncation.
    if let Some(newline) = captured[..end].rfind('\n') {
        if newline + 1 >= available / 2 {
            end = newline + 1;
        }
    }
    if end == 0 {
        return Err("Budget cannot advance this UTF-8 page; increase --budget".into());
    }
    let output = format!(
        "{header}{}{}",
        &captured[..end],
        footer(snapshot, query, offset + end)?
    );
    debug_assert!(output.len() <= budget);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{sha, FileEntry};
    use std::collections::BTreeMap;

    fn snapshot(files: &[(&str, &str)]) -> Snapshot {
        let files: BTreeMap<_, _> = files
            .iter()
            .map(|(path, text)| {
                (
                    path.to_string(),
                    FileEntry {
                        path: path.to_string(),
                        hash: sha(text.as_bytes()),
                        text: text.to_string(),
                    },
                )
            })
            .collect();
        let hash = sha(files
            .values()
            .map(|file| format!("{}:{}", file.path, file.hash))
            .collect::<String>());
        Snapshot {
            hash,
            files,
            omitted: 3,
        }
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }
    fn call(snapshot: &Snapshot, values: &[&str]) -> Result<String> {
        execute(snapshot, parse(&args(values))?)
    }

    fn content(page: &str) -> (&str, Option<&str>) {
        let (_, rest) = page.split_once("\n\n").unwrap();
        if let Some((body, token)) = rest.rsplit_once("\ncontinue ") {
            (body, Some(token.trim_end()))
        } else {
            (rest.strip_suffix("\nend\n").unwrap(), None)
        }
    }

    #[test]
    fn overview_reports_exact_hashes_and_omissions_without_source_bodies() {
        let s = snapshot(&[("lib/code.rs", "PRIVATE_BODY"), ("README.md", "hello")]);
        let page = call(&s, &[]).unwrap();
        assert!(page.contains(&format!("snapshot {}", s.hash)));
        assert!(page.contains("indexed 2 omitted 3"));
        assert!(page.contains(&format!("lib/code.rs hash={}", sha(b"PRIVATE_BODY"))));
        assert!(!page.contains("PRIVATE_BODY"));
    }

    #[test]
    fn literal_search_does_not_invent_ast_edges_or_match_regex() {
        let s = snapshot(&[("code.py", "def hello():\n    # a.b\n    pass\n# axb\n")]);
        let page = call(&s, &["a.b"]).unwrap();
        assert!(page.contains("text matches (literal, case-sensitive)"));
        assert!(page.contains("code.py:2:     # a.b"));
        assert!(!page.contains("axb"));
        assert!(!page.contains("def hello"));
        assert!(call(&s, &["HELLO"]).unwrap().contains("No text matches"));
    }

    #[test]
    fn precise_path_and_line_projection_never_includes_other_lines_or_files() {
        let s = snapshot(&[
            ("pkg/a.py", "hidden\nselected Ω\nselected 😀\nsecret\n"),
            ("other.py", "selected elsewhere"),
        ]);
        let page = call(&s, &["--path", "pkg/a.py", "--start", "2", "--end", "3"]).unwrap();
        assert!(page.contains("2 | selected Ω\n3 | selected 😀"));
        assert!(!page.contains("hidden"));
        assert!(!page.contains("secret"));
        assert!(!page.contains("elsewhere"));
        let search = call(
            &s,
            &["Ω", "--path", "pkg/a.py", "--start", "2", "--end", "3"],
        )
        .unwrap();
        assert!(search.contains("pkg/a.py:2: selected Ω"));
        assert!(!search.contains("selected 😀"));
    }

    #[test]
    fn empty_file_is_still_an_indexed_path_with_its_hash() {
        let s = snapshot(&[("empty.rs", "")]);
        assert!(call(&s, &["--path", "empty.rs"])
            .unwrap()
            .contains("(empty file)"));
        assert!(call(&s, &["--path", "unknown.rs"]).is_err());
    }

    #[test]
    fn unicode_long_lines_and_overview_are_fully_pageable_at_minimum_budget() {
        let long = format!("{}\n{}", "😀漢字λ".repeat(400), "last line\r\n".repeat(200));
        let mut files = vec![("deep/unicode.txt".to_owned(), long)];
        for i in 0..80 {
            files.push((
                format!("directory/file-{i}.rs"),
                "fn main() {}\n".to_owned(),
            ));
        }
        let borrowed = files
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect::<Vec<_>>();
        let s = snapshot(&borrowed);
        for first in [
            args(&["--path", "deep/unicode.txt", "--budget", "1024"]),
            args(&["--budget", "1024"]),
        ] {
            let options = parse(&first).unwrap();
            let expected = render(&s, &options.query).unwrap();
            let mut page = execute(&s, options).unwrap();
            let mut recovered = String::new();
            let mut pages = 0;
            loop {
                assert!(page.len() <= 1024);
                pages += 1;
                assert!(pages < 1_000);
                let (piece, cursor) = content(&page);
                assert!(!piece.is_empty());
                recovered.push_str(piece);
                let Some(cursor) = cursor else {
                    break;
                };
                page = call(&s, &["--cursor", cursor, "--budget", "1024"]).unwrap();
            }
            assert!(pages > 1);
            assert_eq!(recovered, expected);
        }
    }

    #[test]
    fn cursor_round_trip_can_change_budget_but_not_snapshot_or_query() {
        let text = "some long text\n".repeat(500);
        let s = snapshot(&[("code.rs", &text)]);
        let page = call(&s, &["--path", "code.rs", "--budget", "1024"]).unwrap();
        let cursor = content(&page).1.unwrap();
        assert!(call(&s, &["--cursor", cursor, "--budget", "4096"]).is_ok());
        assert!(call(&s, &["--cursor", cursor, "--path", "code.rs"]).is_err());
        let changed = snapshot(&[("code.rs", "changed")]);
        assert!(call(&changed, &["--cursor", cursor])
            .unwrap_err()
            .to_string()
            .contains("Stale"));
    }

    #[test]
    fn forged_offsets_cannot_split_utf8_or_advance_past_projection() {
        let s = snapshot(&[("unicode.txt", "😀 tail")]);
        let query = Query {
            path: Some("unicode.txt".to_owned()),
            ..Query::default()
        };
        let body = render(&s, &query).unwrap();
        let split = body.find('😀').unwrap() + 1;
        for offset in [split, body.len(), usize::MAX] {
            let cursor = encode(&Cursor {
                version: 1,
                snapshot: s.hash.clone(),
                query: query.clone(),
                offset,
            })
            .unwrap();
            assert!(call(&s, &["--cursor", &cursor]).is_err());
        }
    }

    #[test]
    fn malformed_cursors_and_projection_flags_fail_cleanly() {
        let s = snapshot(&[("code.rs", "hello")]);
        for cursor in ["", "x", "00", "7b7d", "💥"] {
            assert!(call(&s, &["--cursor", cursor]).is_err());
        }
        for values in [
            vec!["--budget", "1023"],
            vec!["--budget", "32001"],
            vec!["--start", "1"],
            vec!["--path", "code.rs", "--end", "0"],
            vec!["--path", "code.rs", "--start", "3", "--end", "2"],
            vec!["--path", "../secret"],
            vec!["--unknown"],
            vec!["--path"],
            vec!["--budget", "1024", "--budget", "2048"],
        ] {
            assert!(call(&s, &values).is_err(), "{values:?}");
        }
    }
}
