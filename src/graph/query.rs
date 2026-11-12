use super::{cache, model::*};
use crate::{
    project::{Project, Snapshot},
    Result,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    #[serde(rename = "m")]
    mode: String,
    #[serde(rename = "l")]
    language: String,
    #[serde(rename = "s")]
    selector: String,
    #[serde(rename = "r")]
    relations: Vec<String>,
    #[serde(rename = "d")]
    depth: usize,
    #[serde(rename = "b")]
    body: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    #[serde(rename = "v")]
    snapshot: String,
    #[serde(rename = "g")]
    index: String,
    #[serde(rename = "q")]
    query: Query,
    #[serde(rename = "o")]
    offset: usize,
}
struct Parsed {
    query: Option<Query>,
    cursor: Option<String>,
    budget: usize,
}
fn parse(args: &[String], mode: &str) -> Result<Parsed> {
    let mut selector = Vec::new();
    let mut language = None;
    let mut relations = vec![
        "calls".into(),
        "callers".into(),
        "types".into(),
        "used-by".into(),
    ];
    let mut depth = 1;
    let mut body = false;
    let mut cursor = None;
    let mut budget = 12000;
    let mut seen = BTreeSet::new();
    let mut i = 0;
    while i < args.len() {
        let flag = &args[i];
        if flag == "--body" {
            if !seen.insert(flag.clone()) {
                return Err("Repeated --body".into());
            }
            body = true;
            i += 1;
            continue;
        }
        if flag.starts_with("--") {
            if !seen.insert(flag.clone()) {
                return Err(format!("Repeated {flag}").into());
            }
            let value = args
                .get(i + 1)
                .ok_or_else(|| format!("Missing {flag} value"))?;
            match flag.as_str() {
                "--symbol" => selector.push(value.clone()),
                "--language" => {
                    super::model::language(value)?;
                    language = Some(value.clone());
                }
                "--relations" => relations = value.split(',').map(str::to_string).collect(),
                "--depth" => depth = value.parse().map_err(|_| "--depth requires 1..3")?,
                "--budget" => budget = value.parse().map_err(|_| "Invalid --budget")?,
                "--cursor" => cursor = Some(value.clone()),
                _ => return Err(format!("Unknown semantic option {flag}").into()),
            }
            i += 2;
        } else {
            selector.push(flag.clone());
            i += 1;
        }
    }
    if !(1024..=32000).contains(&budget) {
        return Err("--budget must be 1024..32000 UTF-8 bytes".into());
    }
    if let Some(cursor) = cursor {
        if !selector.is_empty()
            || language.is_some()
            || seen.iter().any(|s| s != "--cursor" && s != "--budget")
        {
            return Err("Resume with --cursor and optional --budget only".into());
        }
        return Ok(Parsed {
            query: None,
            cursor: Some(cursor),
            budget,
        });
    }
    let selector = selector.join(" ");
    if selector.is_empty() || selector.len() > 8192 || selector.contains(['\n', '\r', '\0']) {
        return Err("Provide a symbol selector or find query (max8192 bytes)".into());
    }
    if !(1..=3).contains(&depth) {
        return Err("--depth requires 1..3".into());
    }
    for relation in &relations {
        if !matches!(
            relation.as_str(),
            "calls" | "callers" | "types" | "used-by" | "references" | "inheritance"
        ) {
            return Err(format!("Unknown relation {relation}").into());
        }
    }
    relations.sort();
    relations.dedup();
    let language = language
        .or_else(|| {
            selector
                .split_once(':')
                .and_then(|(prefix, _)| super::model::language(prefix).ok().map(str::to_string))
        })
        .unwrap_or_default();
    Ok(Parsed {
        query: Some(Query {
            mode: mode.into(),
            language,
            selector,
            relations,
            depth,
            body,
        }),
        cursor: None,
        budget,
    })
}
pub fn find(project: &Project, args: &[String]) -> Result<String> {
    execute(project, parse(args, "find")?)
}
pub fn impact(project: &Project, args: &[String]) -> Result<String> {
    execute(project, parse(args, "impact")?)
}
pub fn context(project: &Project, args: &[String]) -> Result<String> {
    execute(project, parse(args, "context")?)
}
fn choose_language(project: &Project, query: &mut Query) -> Result<()> {
    if !query.language.is_empty() {
        super::model::language(&query.language)?;
        return Ok(());
    }
    let available: Vec<_> = LANGUAGES
        .iter()
        .copied()
        .filter(|language| {
            project
                .state
                .join(format!("semantic/{language}.json"))
                .is_file()
        })
        .collect();
    if available.len() != 1 {
        return Err(
            "Specify --language (see graph --help), or use the compact symbol ID printed by graph find"
                .into(),
        );
    }
    query.language = available[0].into();
    Ok(())
}
fn resolve<'a>(cached: &'a Envelope, selector: &str) -> Result<&'a Node> {
    if let Some(node) = cached
        .graph
        .nodes
        .iter()
        .find(|n| n.id == selector || public_id(&cached.spec.language, &n.id) == selector)
    {
        return Ok(node);
    }
    let matches: Vec<_> = cached
        .graph
        .nodes
        .iter()
        .filter(|n| n.qualified_name == selector || n.name == selector)
        .collect();
    if matches.len() == 1 {
        return Ok(matches[0]);
    }
    if matches.is_empty() {
        return Err(
            "Symbol not found in this compiler index. Use graph find QUERY --language LANGUAGE."
                .into(),
        );
    }
    let mut detail = String::from("Ambiguous symbol; choose an exact compact ID:\n");
    for node in matches.iter().take(20) {
        writeln!(
            detail,
            "{} {} {}",
            public_id(&cached.spec.language, &node.id),
            node.qualified_name,
            node.signature
        )?;
    }
    if matches.len() > 20 {
        writeln!(
            detail,
            "{} additional alternatives; narrow graph find",
            matches.len() - 20
        )?;
    }
    Err(detail.into())
}
fn execute(project: &Project, parsed: Parsed) -> Result<String> {
    let snapshot = project.snapshot()?;
    let (mut query, offset, expected) = if let Some(token) = parsed.cursor {
        let cursor = decode(&token)?;
        if cursor.snapshot != snapshot.hash {
            return Err("Stale semantic cursor: indexed sources/config changed".into());
        }
        (cursor.query, cursor.offset, Some(cursor.index))
    } else {
        (parsed.query.ok_or("Missing semantic query")?, 0, None)
    };
    choose_language(project, &mut query)?;
    // Validate byte extents against the exact text we will render.
    let cached = cache::load_at(project, &snapshot, &query.language)?;
    if expected.as_deref().is_some_and(|h| h != cached.index_hash) {
        return Err("Stale semantic cursor: graph rebuilt".into());
    }
    if query.mode != "find" {
        query.selector = public_id(&query.language, &resolve(&cached, &query.selector)?.id);
    }
    let text = render(&snapshot, &cached, &query)?;
    let after = project.snapshot()?;
    if after.hash != snapshot.hash {
        return Err(
            "Sources/config changed while preparing semantic context; retry the query".into(),
        );
    }
    cache::freshness(project, &after, &cached).map_err(|e| {
        format!("Semantic inputs changed while preparing context: {e}; refresh the graph")
    })?;
    page(
        &snapshot.hash,
        &cached.index_hash,
        &query,
        &text,
        offset,
        parsed.budget,
    )
}
fn brief(output: &mut String, cached: &Envelope, node: &Node) -> Result<()> {
    let id = public_id(&cached.spec.language, &node.id);
    let location = node
        .path
        .as_deref()
        .unwrap_or(if node.implicit && !node.external {
            "compiler-synthesized"
        } else {
            "external"
        });
    writeln!(
        output,
        "[{id}] {} {} {}:{} {}",
        node.kind,
        node.qualified_name,
        location,
        node.line,
        if node.implicit {
            "implicit compiler declaration"
        } else if node.definition {
            "definition"
        } else {
            "declaration"
        }
    )?;
    writeln!(output, "  {}", node.signature)?;
    Ok(())
}
fn follow<'a>(
    edge: &'a Edge,
    current: &str,
    query: &Query,
    nodes: &BTreeMap<&str, &Node>,
) -> Option<(&'a str, &'static str)> {
    if query.mode == "impact" {
        return (edge.target == current).then_some((edge.source.as_str(), "impacted-by"));
    }
    for relation in &query.relations {
        match relation.as_str() {
            "calls" if edge.source == current && edge.kind == "calls" => {
                return Some((&edge.target, "calls"))
            }
            "callers" if edge.target == current && edge.kind == "calls" => {
                return Some((&edge.source, "caller"))
            }
            "types"
                if edge.source == current
                    && matches!(edge.kind.as_str(), "uses_type" | "aliases" | "inherits") =>
            {
                return Some((&edge.target, "type"))
            }
            "types"
                if edge.source == current
                    && edge.kind == "contains"
                    && nodes
                        .get(edge.target.as_str())
                        .is_some_and(|n| matches!(n.kind.as_str(), "field" | "alias")) =>
            {
                return Some((&edge.target, "member"))
            }
            "used-by"
                if edge.target == current
                    && matches!(edge.kind.as_str(), "uses_type" | "aliases") =>
            {
                return Some((&edge.source, "type-user"))
            }
            "references" if edge.target == current && edge.kind == "references" => {
                return Some((&edge.source, "reference"))
            }
            "references" if edge.source == current && edge.kind == "references" => {
                return Some((&edge.target, "references"))
            }
            "inheritance" if edge.source == current && edge.kind == "inherits" => {
                return Some((&edge.target, "base"))
            }
            "inheritance" if edge.target == current && edge.kind == "inherits" => {
                return Some((&edge.source, "derived"))
            }
            _ => {}
        }
    }
    None
}
fn render(snapshot: &Snapshot, cached: &Envelope, query: &Query) -> Result<String> {
    let graph = &cached.graph;
    let mut output=format!("analyzer {} {}\nanalysis_complete={} scope=compiler-static nodes={} edges={} unresolved={} diagnostics={}\n",graph.analyzer.name,graph.analyzer.version,graph.analysis_complete,graph.nodes.len(),graph.edges.len(),graph.unresolved.len(),graph.diagnostics.len());
    for limitation in &graph.limitations {
        writeln!(output, "limit: {limitation}")?;
    }
    if !graph.analysis_complete {
        writeln!(output,"PARTIAL: missing relations are not evidence of absence; inspect graph status diagnostics.")?;
    }
    if query.mode == "find" {
        let needle = query.selector.to_lowercase();
        let selected: Vec<_> = graph
            .nodes
            .iter()
            .filter(|n| {
                n.name.to_lowercase().contains(&needle)
                    || n.qualified_name.to_lowercase().contains(&needle)
                    || n.signature.to_lowercase().contains(&needle)
            })
            .collect();
        writeln!(
            output,
            "compiler symbols matching {:?}: {}",
            query.selector,
            selected.len()
        )?;
        for node in selected {
            brief(&mut output, cached, node)?;
        }
        return Ok(output);
    }
    let selected = resolve(cached, &query.selector)?;
    let nodes: BTreeMap<&str, &Node> = graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut visited = BTreeSet::new();
    visited.insert(selected.id.as_str());
    let mut queue = VecDeque::from([(selected.id.as_str(), 0usize)]);
    let mut links = BTreeSet::new();
    while let Some((current, depth)) = queue.pop_front() {
        if depth == query.depth {
            continue;
        }
        for edge in &graph.edges {
            if let Some((next, label)) = follow(edge, current, query, &nodes) {
                links.insert((
                    edge.source.as_str(),
                    edge.target.as_str(),
                    edge.kind.as_str(),
                    edge.path.as_str(),
                    edge.line,
                    edge.column,
                    edge.dispatch.as_str(),
                    label,
                ));
                if visited.insert(next) {
                    queue.push_back((next, depth + 1));
                }
            }
        }
    }
    writeln!(
        output,
        "selected {} depth={} mode={} related={}",
        public_id(&query.language, &selected.id),
        query.depth,
        query.mode,
        visited.len().saturating_sub(1)
    )?;
    let files: BTreeSet<_> = visited
        .iter()
        .filter_map(|id| nodes[id].path.as_deref())
        .collect();
    for path in files {
        if let Some(entry) = snapshot.files.get(path) {
            writeln!(output, "file {path} hash={}", entry.hash)?;
        }
    }
    brief(&mut output, cached, selected)?;
    for id in &visited {
        if *id != selected.id {
            brief(&mut output, cached, nodes[id])?;
        }
    }
    if links.is_empty() {
        writeln!(output, "relations: none in this indexed static scope")?;
    }
    for (source, target, kind, path, line, column, dispatch, label) in links {
        writeln!(
            output,
            "{} --{kind}/{label}--> {} at {path}:{line}:{column} dispatch={dispatch}",
            public_id(&query.language, source),
            public_id(&query.language, target)
        )?;
    }
    let unresolved: Vec<_> = graph
        .unresolved
        .iter()
        .filter(|u| u.source.as_deref().is_some_and(|id| visited.contains(id)))
        .collect();
    for item in unresolved.iter().take(20) {
        writeln!(
            output,
            "unresolved {} {}:{} {:?}: {}",
            item.kind, item.path, item.line, item.expression, item.reason
        )?;
    }
    if unresolved.len() > 20 {
        writeln!(
            output,
            "{} additional unresolved entries; see cached graph/status",
            unresolved.len() - 20
        )?;
    }
    if query.body {
        if selected.external {
            return Err("External symbol source is not in indexed files; only its compiler contract is available".into());
        }
        if selected.implicit {
            return Err("Compiler-synthesized declaration has no user-written body; query its contract without --body".into());
        }
        let entry = snapshot
            .files
            .get(selected.path.as_deref().ok_or("Missing declaration path")?)
            .ok_or("Missing declaration source")?;
        if selected.end_byte <= selected.start_byte {
            return Err(
                "Compiler did not provide a declaration extent; use explicit context --path".into(),
            );
        }
        writeln!(
            output,
            "[source {} bytes={}..{}]",
            public_id(&query.language, &selected.id),
            selected.start_byte,
            selected.end_byte
        )?;
        output.push_str(
            entry
                .text
                .get(selected.start_byte..selected.end_byte)
                .ok_or("Compiler source boundaries changed; refresh the graph")?,
        );
        if !output.ends_with('\n') {
            output.push('\n');
        }
    }
    Ok(output)
}
fn encode(cursor: &Cursor) -> Result<String> {
    let bytes = serde_json::to_vec(cursor)?;
    Ok(format!(
        "g1-{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}
fn decode(token: &str) -> Result<Cursor> {
    let text = token
        .strip_prefix("g1-")
        .ok_or("Invalid graph cursor prefix")?;
    if !text.is_ascii() || text.len() > 65536 || text.len() % 2 != 0 {
        return Err("Invalid graph cursor length".into());
    }
    let bytes = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let cursor: Cursor = serde_json::from_slice(&bytes)?;
    if cursor.query.selector.is_empty()
        || cursor.query.selector.len() > 8192
        || cursor.query.selector.contains(['\n', '\r', '\0'])
        || cursor.query.relations.len() > 6
        || cursor.query.relations.iter().any(|r| {
            !matches!(
                r.as_str(),
                "calls" | "callers" | "types" | "used-by" | "references" | "inheritance"
            )
        })
        || !(1..=3).contains(&cursor.query.depth)
        || !matches!(cursor.query.mode.as_str(), "find" | "context" | "impact")
    {
        return Err("Invalid graph cursor query".into());
    }
    super::model::language(&cursor.query.language)?;
    Ok(cursor)
}
fn page(
    snapshot: &str,
    index: &str,
    query: &Query,
    text: &str,
    offset: usize,
    budget: usize,
) -> Result<String> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return Err("Invalid semantic cursor offset".into());
    }
    let header = format!("snapshot {snapshot}\nindex {index}\nfragment bytes={offset}..\n");
    if header.len() + text.len() - offset + 4 <= budget {
        return Ok(format!("{header}{}end\n", &text[offset..]));
    }
    let reserve = encode(&Cursor {
        snapshot: snapshot.into(),
        index: index.into(),
        query: query.clone(),
        offset: usize::MAX,
    })?
    .len()
        + 11;
    let available = budget
        .checked_sub(header.len() + reserve)
        .ok_or("Budget too small for semantic cursor; increase --budget")?;
    let mut end = (offset + available).min(text.len());
    while end > offset && !text.is_char_boundary(end) {
        end -= 1;
    }
    if end == offset {
        return Err("Budget too small for source content and cursor".into());
    }
    if let Some(newline) = text[offset..end].rfind('\n') {
        if newline > available / 2 {
            end = offset + newline + 1;
        }
    }
    let cursor = encode(&Cursor {
        snapshot: snapshot.into(),
        index: index.into(),
        query: query.clone(),
        offset: end,
    })?;
    let result = format!("{header}{}continue {cursor}\n", &text[offset..end]);
    if result.len() > budget {
        return Err("Semantic paging overhead exceeds budget".into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_cursor_has_complete_budgeted_roundtrip() {
        let q = Query {
            mode: "find".into(),
            language: "cpp".into(),
            selector: "find".into(),
            relations: vec![],
            depth: 1,
            body: false,
        };
        let text = "Σ漢😀 line\n".repeat(1000);
        let mut offset = 0;
        let mut recovered = String::new();
        loop {
            let result = page("source", "index", &q, &text, offset, 1200).unwrap();
            assert!(result.len() <= 1200);
            let start = result.find("fragment bytes=").unwrap();
            let start = result[start..].find('\n').unwrap() + start + 1;
            let final_marker = if result.ends_with("end\n") {
                result.len() - 4
            } else {
                result.rfind("continue ").unwrap()
            };
            recovered.push_str(&result[start..final_marker]);
            if result.ends_with("end\n") {
                break;
            }
            let token = result[final_marker + 9..].trim();
            offset = decode(token).unwrap().offset;
        }
        assert_eq!(recovered, text);
    }
    #[test]
    fn malformed_cursor_and_options_are_rejected() {
        assert!(decode("g1-zz").is_err());
        assert!(decode("g1-0").is_err());
        assert!(decode("g1-漢字").is_err());
        assert!(parse(
            &["--symbol".into(), "a".into(), "--depth".into(), "9".into()],
            "context"
        )
        .is_err());
        assert!(parse(
            &["--cursor".into(), "g1-00".into(), "--body".into()],
            "context"
        )
        .is_err());
        let q = Query {
            mode: "find".into(),
            language: "cpp".into(),
            selector: "x".into(),
            relations: vec![],
            depth: 1,
            body: false,
        };
        assert!(page("s", "i", &q, "😀", 1, 1200).is_err());
        let mut malformed = q.clone();
        malformed.relations.push("invented-edge".into());
        let token = encode(&Cursor {
            snapshot: "s".into(),
            index: "i".into(),
            query: malformed,
            offset: 0,
        })
        .unwrap();
        assert!(decode(&token).is_err());
    }
    #[test]
    fn normal_compact_symbol_query_fits_minimum_page_budget() {
        let q = parse(
            &["--symbol".into(), "cpp:0123456789abcdef".into()],
            "context",
        )
        .unwrap()
        .query
        .unwrap();
        let result = page(
            &"a".repeat(64),
            &"b".repeat(64),
            &q,
            &"long context\n".repeat(100),
            0,
            1024,
        )
        .unwrap();
        assert!(result.len() <= 1024);
        assert!(result.contains("continue g1-"));
    }
}
