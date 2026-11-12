//! Bounded navigation evidence for preserving a subsystem's approach.
//! Paths, lexical matches and static edges suggest where to inspect. They do
//! not establish equivalent responsibilities, ownership or an authoritative style.
use crate::{graph, project::FileEntry, retrieval::NavigationQuery, source_paths::test_path};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Clone, Debug)]
struct Candidate {
    path: String,
    hash: String,
    line: usize,
    role: &'static str,
    reason: String,
    priority: usize,
}

#[derive(Default)]
pub struct Selection {
    pub hints: BTreeMap<String, BTreeSet<usize>>,
    candidates: Vec<Candidate>,
    matching_implementations: usize,
    eligible_implementations: usize,
    omitted: usize,
}

fn language(path: &str) -> Option<&'static str> {
    Some(match Path::new(path).extension()?.to_str()? {
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => "cpp",
        "rs" => "rust",
        "py" | "pyi" => "python",
        "cs" => "csharp",
        "ts" | "tsx" | "mts" | "cts" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "go" => "go",
        "java" => "java",
        "kt" => "kotlin",
        "swift" => "swift",
        "rb" => "ruby",
        "php" => "php",
        "lua" => "lua",
        "sql" => "sql",
        "vue" => "vue",
        "svelte" => "svelte",
        _ => return None,
    })
}
fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn matches(value: &str, search: &[String]) -> usize {
    let lower = value.to_lowercase();
    search.iter().filter(|s| lower.contains(s.as_str())).count()
}
fn line_for(file: &FileEntry, search: &[String]) -> usize {
    file.text
        .lines()
        .position(|line| matches(line, search) > 0)
        .or_else(|| {
            file.text.lines().position(|line| {
                let text = line.trim_start();
                [
                    "fn ",
                    "pub fn ",
                    "def ",
                    "async def ",
                    "class ",
                    "struct ",
                    "interface ",
                    "function ",
                    "impl ",
                ]
                .iter()
                .any(|prefix| text.starts_with(prefix))
            })
        })
        .map_or(1, |n| n + 1)
}
fn test_key(path: &str) -> String {
    let stem = Path::new(path)
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let stem = stem.strip_prefix("test_").unwrap_or(&stem);
    let stem = [".test", ".spec", "_tests", "_test", "tests", "test"]
        .iter()
        .find_map(|s| stem.strip_suffix(s))
        .unwrap_or(stem);
    stem.chars().filter(|c| c.is_alphanumeric()).collect()
}
fn clip(value: &str, limit: usize) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if cleaned.len() <= limit {
        return cleaned;
    }
    let mut end = limit.saturating_sub(3).min(cleaned.len());
    while !cleaned.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &cleaned[..end])
}
impl Selection {
    fn add(
        &mut self,
        file: &FileEntry,
        line: usize,
        role: &'static str,
        reason: String,
        priority: usize,
    ) {
        let line = line.max(1).min(file.text.lines().count().max(1));
        if self
            .candidates
            .iter()
            .any(|c| c.path == file.path && c.role == role)
        {
            return;
        }
        self.hints
            .entry(file.path.clone())
            .or_default()
            .insert(line);
        self.candidates.push(Candidate {
            path: file.path.clone(),
            hash: file.hash.clone(),
            line,
            role,
            reason,
            priority,
        });
    }
    /// 400: query anchor; 300: local example; 250: test/static neighbor; 0: absent.
    /// These are source allocation priorities, not confidence in semantic reuse.
    pub fn priority(&self, path: &str) -> usize {
        self.candidates
            .iter()
            .filter(|c| c.path == path)
            .map(|c| c.priority)
            .max()
            .unwrap_or(0)
    }
    pub fn render(
        &self,
        covered: &BTreeMap<&str, Option<BTreeSet<usize>>>,
        budget: usize,
        hashes: bool,
    ) -> String {
        let header = "\nconsistency candidates: inspect suitability; names/locality are not ownership or equivalent contracts\n";
        let note = if self.matching_implementations == 0 {
            "No matching implementation found in eligible scope; existing capability remains unknown.\n"
        } else {
            "Local examples may be legacy or follow a different contract; compare APIs/errors/effects/tests.\n"
        };
        let trailer = |shown: usize| {
            format!("consistency coverage implementations={} query_matches={} shown={} selection_hits_omitted={}; lexical/static selection is partial\n", self.eligible_implementations, self.matching_implementations, shown, self.omitted + self.candidates.len() - shown)
        };
        if header.len() + note.len() + trailer(0).len() > budget {
            let short = "consistency evidence omitted: budget; suitability unknown\n";
            return if short.len() <= budget {
                short.into()
            } else {
                String::new()
            };
        }
        let mut out = String::from(header);
        out.push_str(note);
        let mut shown = 0;
        // Interleave anchors and their local/test examples rather than allowing
        // every same-name match in distant subsystems to consume the section.
        for c in &self.candidates {
            let visible = match covered.get(c.path.as_str()) {
                Some(None) => "in-packet",
                Some(Some(lines)) if lines.contains(&c.line) => "line-in-packet",
                _ => "line-missing",
            };
            let row = format!(
                "{} @{}:{}{} ({visible}): {}\n",
                c.role,
                c.path,
                c.line,
                if hashes {
                    format!(" hash={}", c.hash)
                } else {
                    String::new()
                },
                clip(&c.reason, 160)
            );
            if out.len() + row.len() + trailer(shown + 1).len() <= budget {
                out.push_str(&row);
                shown += 1;
            }
        }
        out.push_str(&trailer(shown));
        out
    }
}

/// Select only from the caller's admitted scope; never read the filesystem.
/// Envelopes must have passed the caller's full freshness/provenance checks.
/// Input hashes are checked again here before attaching a static relationship.
pub fn select(
    files: &[&FileEntry],
    query: &NavigationQuery,
    envelopes: &[graph::Envelope],
    drivers: &BTreeSet<String>,
) -> Selection {
    let search = query.terms();
    let eligible: BTreeMap<_, _> = files.iter().map(|f| (f.path.as_str(), *f)).collect();
    let implementation: Vec<_> = files
        .iter()
        .copied()
        .filter(|f| {
            language(&f.path).is_some() && !test_path(&f.path) && !drivers.contains(&f.path)
        })
        .collect();
    let mut result = Selection {
        eligible_implementations: implementation.len(),
        ..Selection::default()
    };
    if search.is_empty() {
        return result;
    }
    let evidence: BTreeMap<_, _> = files
        .iter()
        .map(|file| (file.path.as_str(), query.evidence(&file.path, &file.text)))
        .collect();
    let focus = |file: &FileEntry| {
        evidence[file.path.as_str()]
            .focus_lines
            .first()
            .copied()
            .unwrap_or_else(|| line_for(file, search))
    };
    let mut ranked: Vec<_> = implementation
        .iter()
        .copied()
        .filter_map(|f| {
            let signal = &evidence[f.path.as_str()];
            (signal.literal_score > 0 || signal.specificity.0).then_some((signal.rank(), f))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.path.cmp(&b.1.path)));
    result.matching_implementations = ranked.len();
    // One anchor per local language/subsystem leaves room for a distinct
    // neighboring example, rather than spending every slot on the same query.
    let mut groups = BTreeSet::new();
    let anchors: Vec<_> = ranked
        .into_iter()
        .filter_map(|(_, file)| {
            groups
                .insert((parent(&file.path), language(&file.path)))
                .then_some(file)
        })
        .take(3)
        .collect();
    result.omitted += result
        .matching_implementations
        .saturating_sub(anchors.len());
    for anchor in &anchors {
        let signal = &evidence[anchor.path.as_str()];
        result.add(
            anchor,
            focus(anchor),
            "implementation-candidate",
            format!(
                "query: {}exact_ids={} path={} source={}",
                if signal.specificity.0 {
                    "explicit_path=true "
                } else {
                    ""
                },
                signal.specificity.1,
                signal.path_terms,
                signal.source_terms
            ),
            400,
        );
        let mut siblings: Vec<_> = implementation
            .iter()
            .copied()
            .filter(|f| {
                f.path != anchor.path
                    && parent(&f.path) == parent(&anchor.path)
                    && language(&f.path) == language(&anchor.path)
            })
            .collect();
        siblings.sort_by(|a, b| {
            evidence[b.path.as_str()]
                .rank()
                .cmp(&evidence[a.path.as_str()].rank())
                .then(a.path.cmp(&b.path))
        });
        result.omitted += siblings.len().saturating_sub(1);
        if let Some(sibling) = siblings.first() {
            result.add(
                sibling,
                focus(sibling),
                "local-example-candidate",
                format!(
                    "same parent/extension-family as {}; inspect comparable responsibility",
                    clip(&anchor.path, 100)
                ),
                300,
            );
        }
        let key = test_key(&anchor.path);
        let mut tests: Vec<_> = files
            .iter()
            .copied()
            .filter(|f| {
                !drivers.contains(&f.path)
                    && test_path(&f.path)
                    && language(&f.path) == language(&anchor.path)
                    && !key.is_empty()
                    && test_key(&f.path) == key
            })
            .collect();
        tests.sort_by(|a, b| {
            let locality = |path: &str| {
                parent(path)
                    .split('/')
                    .zip(parent(&anchor.path).split('/'))
                    .take_while(|(a, b)| a == b)
                    .count()
            };
            locality(&b.path)
                .cmp(&locality(&a.path))
                .then(a.path.cmp(&b.path))
        });
        result.omitted += tests.len().saturating_sub(1);
        if let Some(test) = tests.first() {
            result.add(
                test,
                focus(test),
                "test-candidate",
                format!(
                    "filename convention matches {}; test discovery/execution unknown",
                    clip(&anchor.path, 100)
                ),
                250,
            );
        }
    }
    for candidate in &result.candidates {
        result
            .hints
            .entry(candidate.path.clone())
            .or_default()
            .extend(
                evidence[candidate.path.as_str()]
                    .focus_lines
                    .iter()
                    .copied(),
            );
    }
    let anchor_paths: BTreeSet<_> = anchors.iter().map(|f| f.path.as_str()).collect();
    let mut graph_rows = 0;
    for envelope in envelopes {
        let hashes: BTreeMap<_, _> = envelope
            .graph
            .inputs
            .iter()
            .map(|i| (i.path.as_str(), i.hash.as_str()))
            .collect();
        let bound = |path: &str| {
            eligible
                .get(path)
                .is_some_and(|f| hashes.get(path).is_some_and(|hash| *hash == f.hash))
        };
        let nodes: BTreeMap<_, _> = envelope
            .graph
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n))
            .collect();
        let seeds: BTreeSet<_> = envelope
            .graph
            .nodes
            .iter()
            .filter(|n| {
                !n.external
                    && n.definition
                    && n.path
                        .as_deref()
                        .is_some_and(|p| anchor_paths.contains(p) && bound(p))
                    && matches(&n.qualified_name, search) > 0
            })
            .map(|n| n.id.as_str())
            .collect();
        let mut edges: Vec<_> = envelope
            .graph
            .edges
            .iter()
            .filter(|e| {
                e.kind != "contains"
                    && (seeds.contains(e.source.as_str()) || seeds.contains(e.target.as_str()))
            })
            .collect();
        edges.sort_by(|a, b| {
            (&a.path, a.line, &a.kind, &a.source, &a.target)
                .cmp(&(&b.path, b.line, &b.kind, &b.source, &b.target))
        });
        for edge in edges {
            let (Some(from), Some(to)) = (
                nodes.get(edge.source.as_str()),
                nodes.get(edge.target.as_str()),
            ) else {
                continue;
            };
            if !bound(&edge.path)
                || from.path.as_deref().is_some_and(|p| !bound(p))
                || to.path.as_deref().is_some_and(|p| !bound(p))
            {
                continue;
            }
            if graph_rows >= 3 {
                result.omitted += 1;
                continue;
            }
            let Some(file) = eligible.get(edge.path.as_str()) else {
                continue;
            };
            if edge.line == 0 || edge.line > file.text.lines().count() {
                continue;
            }
            result.add(
                file,
                edge.line,
                if test_path(&edge.path) {
                    "graph-test-candidate"
                } else {
                    "static-relation-candidate"
                },
                format!(
                    "{} {} -> {} ({}, {})",
                    edge.kind,
                    clip(&from.qualified_name, 40),
                    clip(&to.qualified_name, 40),
                    edge.dispatch,
                    if envelope.graph.analysis_complete {
                        "complete static graph"
                    } else {
                        "partial static graph"
                    }
                ),
                250,
            );
            graph_rows += 1;
        }
    }
    result
}
