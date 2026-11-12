//! A bounded work packet. Small repositories need sources, not a graph ritual.
use crate::{
    graph,
    project::{normalize_path, scope_contains, FileEntry, Project},
    source_paths::test_path,
    Result,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    path::Path,
};

const HELP: &str = "aienv brief [QUERY WORDS] [--path SOURCE_PREFIX]* [--hashes] [--impact] [--budget BYTES]\nOne source packet with configured checks and an existing HANDOFF.md. Small source sets are read directly; larger sets use literal matches and available fresh compiler graphs to select sources. --hashes includes exact file hashes for native CAS edits. --impact adds bounded implementation/local-example/test candidates with selection reasons; larger packets also include a caller/type/reuse map. Requires query words; candidates do not establish ownership or equivalent contracts. Existing source-stale graphs may refresh with recorded options (20s shared deadline, at most 2 attempts); missing graphs are never built. Configuration/provenance changes or refresh failures fall back to lexical selection with a reason. Small packets still bypass graph reads. Tight budgets may omit candidate metadata while retaining source; omissions are explicit. Omitted files/lines are explicit; use ordinary reads or context --path for more. Budget: 2048..32000 UTF-8 bytes (default 16000).\n";

fn source(path: &str) -> bool {
    matches!(
        Path::new(path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "c" | "cc"
            | "cpp"
            | "cxx"
            | "h"
            | "hh"
            | "hpp"
            | "hxx"
            | "cs"
            | "rs"
            | "py"
            | "pyi"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "go"
            | "java"
            | "kt"
            | "swift"
            | "rb"
            | "php"
            | "lua"
            | "sql"
            | "vue"
            | "svelte"
    )
}
fn terms(query: &str) -> Vec<String> {
    let mut terms: Vec<_> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| s.len() >= 2)
        .map(str::to_lowercase)
        .collect();
    terms.sort();
    terms.dedup();
    terms
}
fn matches(value: &str, terms: &[String]) -> usize {
    let lower = value.to_lowercase();
    terms.iter().filter(|s| lower.contains(s.as_str())).count()
}
fn configuration(path: &str) -> bool {
    matches!(
        path,
        "Cargo.toml"
            | "pyproject.toml"
            | "tsconfig.json"
            | "jsconfig.json"
            | "package.json"
            | "CMakeLists.txt"
    ) || path.ends_with(".csproj")
}

// These are presentation groups, not semantic claims about a file's role.
// Keep test/decision text available without letting a long matching test file
// consume every byte before the implementation or its callers can be shown.
fn packet_group(path: &str, drivers: &BTreeSet<String>) -> usize {
    if drivers.contains(path) || path == "HANDOFF.md" {
        2
    } else if test_path(path) {
        1
    } else {
        0
    }
}

fn allocations(available: usize, needs: [usize; 3]) -> [usize; 3] {
    let weights = [3, 1, 1];
    let mut limits = [0; 3];
    let mut left = available;
    // Each round either exhausts the payload budget or satisfies another group.
    // Redistribute space when e.g. a test-only scope has no implementation files.
    for _ in 0..4 {
        let total: usize = (0..3)
            .filter(|&i| limits[i] < needs[i])
            .map(|i| weights[i])
            .sum();
        if total == 0 || left == 0 {
            break;
        }
        let mut assigned = 0;
        for i in 0..3 {
            if limits[i] < needs[i] {
                let part = (left * weights[i] / total)
                    .max(1)
                    .min(needs[i] - limits[i])
                    .min(left - assigned);
                limits[i] += part;
                assigned += part;
            }
        }
        left -= assigned;
    }
    limits
}

fn block_header(file: &FileEntry, hashes: bool) -> String {
    format!(
        "\n[{}:1..{}{}]\n",
        file.path,
        file.text.lines().count(),
        if hashes {
            format!(" hash={}", file.hash)
        } else {
            String::new()
        }
    )
}

fn block_size(file: &FileEntry, hashes: bool) -> usize {
    block_header(file, hashes).len() + file.text.len() + usize::from(!file.text.ends_with('\n'))
}

pub fn run(project: &Project, args: &[String]) -> Result<String> {
    if args == ["--help"] || args == ["-h"] {
        return Ok(HELP.into());
    }
    let mut budget = 16000;
    let mut words = Vec::new();
    let mut paths = Vec::new();
    let mut seen = false;
    let mut hashes = false;
    let mut impact = false;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--" {
            words.extend(args[i + 1..].iter().map(String::as_str));
            break;
        } else if args[i] == "--impact" {
            if impact {
                return Err("Repeated --impact".into());
            }
            impact = true;
        } else if args[i] == "--hashes" {
            if hashes {
                return Err("Repeated --hashes".into());
            }
            hashes = true;
        } else if args[i] == "--budget" {
            if seen {
                return Err("Repeated --budget".into());
            }
            seen = true;
            i += 1;
            budget = args.get(i).ok_or("Missing --budget")?.parse::<usize>()?;
        } else if args[i] == "--path" {
            i += 1;
            let value = args.get(i).ok_or("Missing --path")?;
            paths.push(if value == "." {
                value.clone()
            } else {
                normalize_path(value)?
            });
        } else if args[i].starts_with('-') {
            return Err("Use brief [QUERY] [--budget BYTES]".into());
        } else {
            words.push(args[i].as_str());
        }
        i += 1;
    }
    let query = words.join(" ");
    if impact && terms(&query).is_empty() {
        return Err("--impact requires query words (at least two characters)".into());
    }
    paths.sort();
    paths.dedup();
    if paths.len() > 64 || paths.iter().map(String::len).sum::<usize>() > 2048 {
        return Err("Brief paths exceed limit".into());
    }
    if !(2048..=32000).contains(&budget)
        || query.len() > 2048
        || query.chars().any(char::is_control)
    {
        return Err(
            "Brief requires a single-line query <=2048 bytes and budget 2048..32000".into(),
        );
    }
    let snapshot = project.snapshot()?;
    let config = project.config()?;
    // Show small local check drivers even with a source scope. Their contents
    // reveal whether the named check already builds/runs tests. Never read an
    // argv path that was not admitted by the ordinary project snapshot.
    let drivers: BTreeSet<_> = config
        .checks
        .values()
        .flat_map(|c| &c.argv)
        .filter_map(|arg| {
            let path = Path::new(arg);
            let absolute = if path.is_absolute() {
                Some(path.canonicalize().ok()?)
            } else {
                None
            };
            let relative = if let Some(absolute) = &absolute {
                absolute.strip_prefix(&project.root).ok()?
            } else {
                path
            };
            let mut parts = Vec::new();
            for part in relative.components() {
                match part {
                    std::path::Component::Normal(value) => parts.push(value.to_str()?),
                    std::path::Component::CurDir => (),
                    _ => return None,
                }
            }
            let key = parts.join("/");
            let file = snapshot.files.get(&key)?;
            (file.text.len() <= 4096 && (source(&key) || key.ends_with(".sh"))).then_some(key)
        })
        .collect();
    let navigation = crate::retrieval::NavigationQuery::new(&query);
    let search = navigation.terms();
    let files: Vec<_> = snapshot
        .files
        .values()
        .filter(|f| {
            f.path == "HANDOFF.md"
                || drivers.contains(&f.path)
                || ((source(&f.path) || configuration(&f.path))
                    && (paths.is_empty() || paths.iter().any(|p| scope_contains(p, &f.path))))
        })
        .collect();
    let evidence: BTreeMap<_, _> = files
        .iter()
        .map(|f| (f.path.as_str(), navigation.evidence(&f.path, &f.text)))
        .collect();
    let total: usize = files
        .iter()
        .map(|f| f.text.len() + f.path.len() + if hashes { 104 } else { 32 })
        .sum();
    let small = total + 1500 <= budget;
    let impact_budget = if impact && !small {
        (budget / 6).min(1800)
    } else {
        0
    };
    let mut hints: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut envelopes = Vec::new();
    let mut warnings = Vec::new();
    let mut refresh = graph::RefreshBudget::new();
    // Never pay backend/provenance hashing or an analysis round trip when all
    // eligible source fits. Large packets use only validated cached facts.
    if !small && !search.is_empty() {
        for language in graph::model::LANGUAGES {
            if !project
                .state
                .join(format!("semantic/{language}.json"))
                .is_file()
            {
                continue;
            }
            let loaded = if impact {
                graph::load_for_brief(project, &snapshot, language, &mut refresh)
            } else {
                graph::load_at(project, &snapshot, language).map(|cached| (cached, false))
            };
            match loaded {
                Ok((cached, refreshed)) => {
                    let seeds: BTreeSet<_> = cached
                        .graph
                        .nodes
                        .iter()
                        .filter(|n| !n.external && matches(&n.qualified_name, &search) > 0)
                        .map(|n| n.id.as_str())
                        .collect();
                    let mut selected = seeds.clone();
                    for e in &cached.graph.edges {
                        if seeds.contains(e.source.as_str()) {
                            selected.insert(&e.target);
                        }
                        if seeds.contains(e.target.as_str()) {
                            selected.insert(&e.source);
                        }
                    }
                    for n in &cached.graph.nodes {
                        if !n.external && selected.contains(n.id.as_str()) {
                            if let Some(path) = &n.path {
                                hints.entry(path.clone()).or_default().insert(n.line.max(1));
                            }
                        }
                    }
                    warnings.push(format!("graph {language}: {} static; unresolved={}; one-hop selection, not exhaustive impact{}", if cached.graph.analysis_complete {"complete"} else {"PARTIAL"}, cached.graph.unresolved.len(), if refreshed {"; refreshed source changes"} else {""}));
                    envelopes.push(cached);
                }
                Err(error) => {
                    let reason: String = error
                        .to_string()
                        .chars()
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .take(120)
                        .collect();
                    warnings.push(format!("graph {language}: lexical selection only; {reason}; manual refresh/rebuild needed"));
                }
            }
        }
    }
    let consistency =
        impact.then(|| crate::consistency::select(&files, &navigation, &envelopes, &drivers));
    let consistency_budget = if !impact {
        0
    } else if budget < 4096 {
        // A short explicit omission keeps tight packets useful for source.
        96
    } else {
        (budget / 8).min(1400)
    };
    if let Some(selection) = &consistency {
        for (path, lines) in &selection.hints {
            hints.entry(path.clone()).or_default().extend(lines);
        }
    }
    let mut ranked: Vec<_> = files
        .iter()
        .map(|f| {
            let score = if f.path == "HANDOFF.md" || drivers.contains(&f.path) {
                10000
            } else {
                evidence[f.path.as_str()].literal_score
                    + if hints.contains_key(&f.path) { 50 } else { 0 }
                    + if f.path.starts_with("src/") { 2 } else { 0 }
            };
            (score, *f)
        })
        .collect();
    ranked.sort_by(|a, b| {
        let group_order = if small {
            std::cmp::Ordering::Equal
        } else {
            packet_group(&a.1.path, &drivers).cmp(&packet_group(&b.1.path, &drivers))
        };
        group_order
            .then_with(|| {
                if small {
                    std::cmp::Ordering::Equal
                } else {
                    consistency
                        .as_ref()
                        .map_or(std::cmp::Ordering::Equal, |selection| {
                            selection
                                .priority(&b.1.path)
                                .cmp(&selection.priority(&a.1.path))
                        })
                }
            })
            .then_with(|| {
                if small {
                    std::cmp::Ordering::Equal
                } else {
                    evidence[b.1.path.as_str()]
                        .specificity
                        .cmp(&evidence[a.1.path.as_str()].specificity)
                }
            })
            .then_with(|| {
                if small {
                    std::cmp::Ordering::Equal
                } else {
                    drivers
                        .contains(&b.1.path)
                        .cmp(&drivers.contains(&a.1.path))
                }
            })
            .then(b.0.cmp(&a.0))
            .then(a.1.path.cmp(&b.1.path))
    });
    let mut out = format!(
        "snapshot {}\nbrief {} eligible_files={} indexed_omitted={}\n",
        snapshot.hash,
        if small { "sources" } else { "selected sources" },
        files.len(),
        snapshot.omitted
    );
    if !paths.is_empty() {
        writeln!(
            out,
            "scope {} (HANDOFF.md and small indexed check drivers also included when present)",
            paths.join(",")
        )?;
    }
    for (name, check) in config.checks {
        let line = format!("check {name}: {}\n", serde_json::to_string(&check.argv)?);
        if out.len() + line.len() < budget / 4 {
            out.push_str(&line);
        } else {
            out.push_str("additional checks omitted; see .ai-env/config.json\n");
            break;
        }
    }
    // Graph status is context metadata, not a reason to omit every source line
    // from a tight packet when several configured analyzers are unavailable.
    let warning_limit = (budget / 4)
        .min(1800)
        .min((budget / 3).saturating_sub(out.len()));
    let warning_start = out.len();
    let mut shown = 0;
    for warning in &warnings {
        let omitted = warnings.len() - shown - 1;
        let summary = if omitted > 0 {
            format!("graph metadata omitted={omitted}; inspect graph status; manual refresh/rebuild for failures\n")
        } else {
            String::new()
        };
        if out.len() - warning_start + warning.len() + 1 + summary.len() > warning_limit {
            break;
        }
        writeln!(out, "{warning}")?;
        shown += 1;
    }
    if shown < warnings.len() {
        writeln!(
            out,
            "graph metadata omitted={}; inspect graph status; manual refresh/rebuild for failures",
            warnings.len() - shown
        )?;
    }
    if !small {
        out.push_str("selection balances implementation, tests and check/handoff context by path convention; not semantic coverage\n");
    }
    let mut needs = [0; 3];
    let mut counts = [0; 3];
    for (_, file) in &ranked {
        let group = packet_group(&file.path, &drivers);
        needs[group] += block_size(file, hashes);
        counts[group] += 1;
    }
    let limits = allocations(
        budget.saturating_sub(out.len() + 640 + impact_budget + consistency_budget),
        needs,
    );
    let mut used = [0; 3];
    let mut omitted = Vec::new();
    let mut partial = 0;
    let eligible: BTreeSet<_> = files.iter().map(|f| f.path.as_str()).collect();
    let mut covered = BTreeMap::new();
    for (_, f) in ranked {
        let group = packet_group(&f.path, &drivers);
        let header = block_header(f, hashes);
        let block_len = header.len() + f.text.len() + usize::from(!f.text.ends_with('\n'));
        let remaining = budget.saturating_sub(out.len() + 640 + impact_budget + consistency_budget);
        let allowance = if small {
            remaining
        } else {
            let per_file = if group != 2 && counts[group] > 1 {
                // Reserve source space for a local counterpart as well as
                // query matches; two long anchors must not consume the group.
                (limits[group] / if consistency.is_some() { 4 } else { 2 })
                    .max(160)
                    .max(limits[group].saturating_sub(needs[group].saturating_sub(block_len)))
            } else {
                limits[group]
            };
            remaining
                .min(limits[group].saturating_sub(used[group]))
                .min(per_file)
        };
        if block_len <= allowance {
            out.push_str(&header);
            out.push_str(&f.text);
            if !f.text.ends_with('\n') {
                out.push('\n');
            }
            used[group] += block_len;
            covered.insert(f.path.as_str(), None);
        } else {
            let (excerpt, lines) = excerpt(
                f,
                &search,
                &evidence[f.path.as_str()].focus_lines,
                hints.get(&f.path),
                allowance,
                hashes,
            );
            if !excerpt.is_empty() {
                used[group] += excerpt.len();
                out.push_str(&excerpt);
                covered.insert(f.path.as_str(), Some(lines));
                partial += 1;
            }
            omitted.push(f.path.as_str());
        }
    }
    // The packet is a projection of this exact source snapshot. Never attach
    // old compiler facts or optimistic success claims to different source.
    if project.snapshot()?.hash != snapshot.hash {
        return Err("Sources/config changed while preparing brief; retry".into());
    }
    for cached in &envelopes {
        graph::freshness(project, &snapshot, cached)?;
    }
    if impact_budget > 0 {
        out.push_str(&impact_map(
            &envelopes,
            &search,
            &eligible,
            &covered,
            impact_budget,
        ));
    }
    if let Some(selection) = consistency {
        out.push_str(&selection.render(&covered, consistency_budget, hashes));
    }
    writeln!(out,"\ncoverage full={} partial={} not_full={} other_indexed={} (selection is not exhaustive semantic coverage)", files.len()-omitted.len(), partial, omitted.len(), snapshot.files.len()-files.len())?;
    if !omitted.is_empty() {
        out.push_str("not_full:");
        let mut count = 0;
        for path in &omitted {
            if out.len() + path.len() + 160 > budget {
                break;
            }
            write!(out, " {path}")?;
            count += 1;
        }
        if count < omitted.len() {
            write!(out, " (+{} paths)", omitted.len() - count)?;
        }
        out.push_str("\nRead more with ordinary tools or context --path FILE.\n");
    }
    if out.len() > budget {
        return Err(
            "Brief metadata exceeds budget; narrow repository scope or increase budget".into(),
        );
    }
    Ok(out)
}

fn excerpt(
    file: &FileEntry,
    terms: &[String],
    focused: &[usize],
    hints: Option<&BTreeSet<usize>>,
    budget: usize,
    hashes: bool,
) -> (String, BTreeSet<usize>) {
    if budget < 160 {
        return (String::new(), BTreeSet::new());
    }
    let lines: Vec<_> = file.text.lines().collect();
    let header = format!(
        "\n[{} excerpt{}; omitted lines require source inspection]\n",
        file.path,
        if hashes {
            format!(" hash={}", file.hash)
        } else {
            String::new()
        }
    );
    let mut selected = BTreeSet::new();
    let mut used = header.len();
    let generic = lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| (matches(line, terms) > 0).then_some(i + 1));
    let mut centers = focused
        .iter()
        .copied()
        .chain(hints.into_iter().flatten().copied())
        .chain(generic)
        .peekable();
    if centers.peek().is_none() {
        for (n, line) in lines.iter().enumerate().take(12) {
            let cost = (n + 1).to_string().len() + 1 + line.len() + 1;
            if used + cost > budget {
                break;
            }
            selected.insert(n);
            used += cost;
        }
    } else {
        let mut admit = |line: usize| {
            if selected.contains(&line) {
                return;
            }
            let cost = (line + 1).to_string().len() + 1 + lines[line].len() + 1;
            if used + cost <= budget {
                selected.insert(line);
                used += cost;
            }
        };
        // Reserve the bounded strongest lines before the first window's
        // surrounding comments can consume the whole per-file allocation.
        for center in focused {
            if *center > 0 && *center <= lines.len() {
                admit(*center - 1);
            }
        }
        let mut seen = BTreeSet::new();
        for center in centers {
            if center == 0 || center > lines.len() || !seen.insert(center) {
                continue;
            }
            let n = center - 1;
            // Allocate exact evidence before context, then display admitted
            // lines in source order. Early generic matches cannot evict a late
            // contract merely because its declaration has a larger line number.
            for line in std::iter::once(n)
                .chain(n.saturating_sub(3)..n)
                .chain(n + 1..=(n + 5).min(lines.len() - 1))
            {
                admit(line);
            }
        }
    }
    let mut out = header;
    let mut emitted = BTreeSet::new();
    for n in selected {
        out.push_str(&format!("{}:{}\n", n + 1, lines[n]));
        emitted.insert(n + 1);
    }
    if emitted.is_empty() {
        (String::new(), emitted)
    } else {
        (out, emitted)
    }
}

fn clipped(value: &str, limit: usize) -> String {
    let value: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if value.len() <= limit {
        value
    } else {
        let mut end = limit.saturating_sub(3);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &value[..end])
    }
}

fn impact_row(
    rows: &mut BTreeSet<(u8, String)>,
    candidates: &mut usize,
    priority: u8,
    text: String,
) {
    *candidates += 1;
    rows.insert((priority, text));
    // Reserve independent bounded slots for callers, types, reuse candidates,
    // unresolved sites and other relations. A broad query must not let many
    // candidates in one category evict all evidence in another (60 rows max).
    if rows
        .iter()
        .filter(|(category, _)| *category == priority)
        .count()
        > 12
    {
        if let Some(last) = rows
            .iter()
            .rev()
            .find(|(category, _)| *category == priority)
            .cloned()
        {
            rows.remove(&last);
        }
    }
}

fn impact_map(
    envelopes: &[graph::Envelope],
    search: &[String],
    eligible: &BTreeSet<&str>,
    covered: &BTreeMap<&str, Option<BTreeSet<usize>>>,
    budget: usize,
) -> String {
    let visibility = |path: &str, line: usize| {
        if !eligible.contains(path) {
            "outside-scope"
        } else {
            match covered.get(path) {
                Some(None) => "in-packet",
                Some(Some(lines)) if lines.contains(&line) => "line-in-packet",
                _ => "line-missing",
            }
        }
    };
    let mut rows = BTreeSet::new();
    let mut total_rows = 0;
    let mut seeds_total = 0;
    let mut relations_total = 0;
    let mut unresolved_total = 0;
    let mut linked_test_paths = BTreeSet::new();
    let mut observed_test_paths = BTreeSet::new();
    // Only identities matching the task inside its source scope are seeds.
    // Neighbors may cross that scope: those locations are explicit read gaps,
    // never implied permission to expand a developer's write contract.
    for cached in envelopes {
        let nodes: BTreeMap<_, _> = cached
            .graph
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n))
            .collect();
        let seeds: BTreeSet<_> = cached
            .graph
            .nodes
            .iter()
            .filter(|n| {
                !n.external
                    && n.definition
                    && n.kind != "namespace"
                    && n.path
                        .as_deref()
                        .is_some_and(|path| eligible.contains(path))
                    && matches(&n.name, search) > 0
            })
            .map(|n| n.id.as_str())
            .collect();
        seeds_total += seeds.len();
        for id in &seeds {
            let node = nodes[id];
            let path = node.path.as_deref().unwrap_or("");
            impact_row(
                &mut rows,
                &mut total_rows,
                2,
                format!(
                    "reuse-candidate {} {} @{}:{} [{}]\n",
                    node.kind,
                    clipped(&node.qualified_name, 100),
                    clipped(path, 160),
                    node.line,
                    visibility(path, node.line)
                ),
            );
        }
        for edge in &cached.graph.edges {
            if edge.kind == "contains"
                || !(seeds.contains(edge.source.as_str()) || seeds.contains(edge.target.as_str()))
            {
                continue;
            }
            relations_total += 1;
            if test_path(&edge.path) {
                linked_test_paths.insert(edge.path.as_str());
            }
            let from = nodes[edge.source.as_str()];
            let to = nodes[edge.target.as_str()];
            // Prefer callers (the contract's consumers), then type relations,
            // then outgoing calls/references. Show the actual edge site.
            let priority = if edge.kind == "calls" && seeds.contains(edge.target.as_str()) {
                0
            } else if matches!(edge.kind.as_str(), "uses_type" | "inherits" | "aliases") {
                1
            } else {
                4
            };
            impact_row(
                &mut rows,
                &mut total_rows,
                priority,
                format!(
                    "{}{} {} -> {} @{}:{} [{}; {}]\n",
                    edge.kind,
                    if test_path(&edge.path) {
                        "(test-path)"
                    } else {
                        ""
                    },
                    clipped(&from.qualified_name, 80),
                    clipped(&to.qualified_name, 80),
                    clipped(&edge.path, 160),
                    edge.line,
                    visibility(&edge.path, edge.line),
                    edge.dispatch
                ),
            );
        }
        for gap in &cached.graph.unresolved {
            if gap.source.as_deref().is_some_and(|id| seeds.contains(id)) {
                unresolved_total += 1;
                impact_row(
                    &mut rows,
                    &mut total_rows,
                    3,
                    format!(
                        "unresolved {} @{}:{}: {}\n",
                        gap.kind,
                        clipped(&gap.path, 160),
                        gap.line,
                        clipped(&gap.reason, 100)
                    ),
                );
            }
        }
        for node in &cached.graph.nodes {
            if let Some(path) = node.path.as_deref() {
                if !node.external && test_path(path) {
                    observed_test_paths.insert(path);
                }
            }
        }
    }
    let mut out = String::from("\nimpact: cached static one-hop evidence; not exhaustive\n");
    let mut shown = 0;
    let mut categories: BTreeMap<_, std::collections::VecDeque<_>> = BTreeMap::new();
    for (category, row) in rows {
        categories.entry(category).or_default().push_back(row);
    }
    // Give each available category a turn within the same byte budget.
    // Callers and types go first; a long seed list cannot consume the map.
    while categories.values().any(|rows| !rows.is_empty()) {
        for rows in categories.values_mut() {
            if let Some(row) = rows.pop_front() {
                if out.len() + row.len() + 180 <= budget {
                    out.push_str(&row);
                    shown += 1;
                }
            }
        }
    }
    let trailer = format!("impact coverage seeds={seeds_total} relations={relations_total} unresolved={unresolved_total} rows_omitted={} test_paths_linked={}/{} (path convention; not test results)\n", total_rows-shown, linked_test_paths.len(), observed_test_paths.len());
    if out.len() + trailer.len() <= budget {
        out.push_str(&trailer);
    }
    if envelopes.is_empty() {
        let note =
            "No fresh cached graph: caller/type/test relationships unknown; lexical source only.\n";
        if out.len() + note.len() <= budget {
            out.push_str(note);
        }
    }
    out
}
