//! Deterministic lexical projection for navigation, never a replacement for the task.
use crate::{json, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
};

pub const MAX_QUERY_BYTES: usize = 2048;
const MAX_TASK_BYTES: usize = 65536;
const WINDOWS: usize = 16;
const MAX_TERMS: usize = 64;

/// Shared lexical navigation signals. Exact tokens and paths are evidence for
/// inspection, not proof of equivalent responsibilities or semantic ownership.
#[derive(Clone, Debug)]
pub struct NavigationQuery {
    terms: Vec<String>,
    structured: BTreeSet<String>,
    paths: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Evidence {
    /// Explicit canonical file path, then distinct whole case-sensitive structured identifiers.
    pub specificity: (bool, usize),
    /// Original path*100 + source*10 substring score, retained as fallback.
    pub literal_score: usize,
    pub path_terms: usize,
    pub source_terms: usize,
    /// One-based lexical evidence lines, strongest first, at most sixteen.
    pub focus_lines: Vec<usize>,
}
impl Evidence {
    pub fn rank(&self) -> (bool, usize, usize) {
        (self.specificity.0, self.specificity.1, self.literal_score)
    }
}
fn identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}
fn literal_matches(value: &str, terms: &[String]) -> usize {
    let lower = value.to_lowercase();
    terms
        .iter()
        .filter(|term| lower.contains(term.as_str()))
        .count()
}
fn declaration_shaped(line: &str) -> bool {
    // A lexical presentation preference only; compiler identity remains separate.
    let trimmed = line.trim_start();
    [
        "def ",
        "async def ",
        "class ",
        "struct ",
        "interface ",
        "enum ",
        "trait ",
        "fn ",
        "pub fn ",
        "pub(crate) fn ",
        "pub struct ",
        "pub enum ",
        "func ",
        "type ",
        "function ",
        "export function ",
        "export class ",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix))
}
impl NavigationQuery {
    /// The caller validates the CLI's existing query-size and character limits.
    /// No paths are opened or added to the caller's admitted source scope.
    pub fn new(query: &str) -> Self {
        let words: Vec<_> = query
            .split(|ch| !identifier_char(ch))
            .filter(|word| word.len() >= 2)
            .collect();
        let mut terms: Vec<_> = words.iter().map(|word| word.to_lowercase()).collect();
        terms.sort();
        terms.dedup();
        let structured = words
            .iter()
            .filter(|word| {
                word.len() >= 3
                    && (word.contains('_')
                        || word.chars().skip(1).any(char::is_uppercase)
                        || word.chars().any(char::is_numeric))
            })
            .map(|word| (*word).to_string())
            .collect();
        let paths = query
            .split_whitespace()
            .filter_map(|raw| {
                let path = raw
                    .trim_start_matches(|ch: char| {
                        matches!(ch, '`' | '\'' | '"' | '(' | '[' | '{' | '<')
                    })
                    .trim_end_matches(|ch: char| {
                        matches!(
                            ch,
                            '`' | '\'' | '"' | ')' | ']' | '}' | '>' | ',' | ';' | ':' | '.'
                        )
                    });
                if !(path.contains('/') || path.contains('.')) {
                    return None;
                }
                let normalized = crate::project::normalize_path(path).ok()?;
                (normalized == path).then_some(normalized)
            })
            .collect();
        Self {
            terms,
            structured,
            paths,
        }
    }
    pub fn terms(&self) -> &[String] {
        &self.terms
    }
    pub fn evidence(&self, path: &str, text: &str) -> Evidence {
        let path_terms = literal_matches(path, &self.terms);
        let source_terms = literal_matches(text, &self.terms);
        let mut hits = BTreeSet::new();
        let mut lines = Vec::new();
        if !self.structured.is_empty() {
            for (index, line) in text.lines().enumerate() {
                let line_hits: BTreeSet<_> = line
                    .split(|ch| !identifier_char(ch))
                    .filter(|word| self.structured.contains(*word))
                    .collect();
                if !line_hits.is_empty() {
                    hits.extend(line_hits.iter().copied());
                    lines.push((declaration_shaped(line), line_hits.len(), index + 1));
                }
            }
        }
        lines.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
        Evidence {
            specificity: (self.paths.contains(path), hits.len()),
            literal_score: path_terms * 100 + source_terms * 10,
            path_terms,
            source_terms,
            focus_lines: lines
                .into_iter()
                .take(16)
                .map(|(_, _, line)| line)
                .collect(),
        }
    }
}

#[derive(Clone)]
struct Term {
    text: String,
    key: String,
    position: usize,
    frequency: usize,
    structured: bool,
}

/// Preserve precise overrides; derive a bounded, diverse lexical query otherwise.
/// Selection samples the entire task, not its prefix. It cannot infer semantic importance.
pub fn project(task: &str, precise: Option<&str>) -> Result<String> {
    if task.is_empty() || task.len() > MAX_TASK_BYTES || task.contains('\0') {
        return Err("Task must contain 1..65536 UTF-8 bytes without NUL".into());
    }
    if let Some(query) = precise {
        let query = query.trim();
        if query.is_empty() || query.len() > MAX_QUERY_BYTES || query.chars().any(char::is_control)
        {
            return Err("Explicit query must be a nonempty single line <=2048 UTF-8 bytes".into());
        }
        return Ok(query.into());
    }
    let normalized = task.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.len() <= MAX_QUERY_BYTES && !normalized.chars().any(char::is_control) {
        return Ok(normalized);
    }

    // Token boundaries match brief's literal search. Keep whole Unicode words and
    // identifiers; punctuation/path separators would be split by brief anyway.
    let mut unique: BTreeMap<String, Term> = BTreeMap::new();
    let mut start = None;
    for (offset, ch) in task
        .char_indices()
        .chain(std::iter::once((task.len(), ' ')))
    {
        if ch.is_alphanumeric() || ch == '_' {
            start.get_or_insert(offset);
        } else if let Some(begin) = start.take() {
            let word = &task[begin..offset];
            // brief ignores one-byte terms. Long atomic identifiers are omitted,
            // never split into plausible but nonexistent symbols.
            if !(2..=128).contains(&word.len()) {
                continue;
            }
            let key = word.to_lowercase();
            let structured = word.contains('_')
                || word.chars().skip(1).any(char::is_uppercase)
                || word.chars().any(char::is_numeric)
                || [begin.checked_sub(1), Some(offset)]
                    .into_iter()
                    .flatten()
                    .any(|at| matches!(task.as_bytes().get(at), Some(b'`' | b'/' | b'\\')));
            unique
                .entry(key.clone())
                .and_modify(|t| t.frequency += 1)
                .or_insert(Term {
                    text: word.into(),
                    key,
                    position: begin,
                    frequency: 1,
                    structured,
                });
        }
    }
    let mut windows: Vec<Vec<Term>> = vec![vec![]; WINDOWS];
    for term in unique.into_values() {
        let window = (term.position * WINDOWS / task.len()).min(WINDOWS - 1);
        windows[window].push(term);
    }
    for (index, window) in windows.iter_mut().enumerate() {
        window.sort_by(|a, b| {
            b.structured
                .cmp(&a.structured)
                .then(a.frequency.cmp(&b.frequency))
                .then(b.text.chars().count().cmp(&a.text.chars().count()))
                .then_with(|| {
                    if index == WINDOWS - 1 {
                        b.position.cmp(&a.position)
                    } else {
                        a.position.cmp(&b.position)
                    }
                })
                .then(a.key.cmp(&b.key))
        });
    }
    // Cover both ends and the middle before filling nearby windows. No raw byte
    // slicing: a term belongs to the window containing its original start.
    const ORDER: [usize; WINDOWS] = [0, 15, 8, 4, 12, 2, 6, 10, 14, 1, 3, 5, 7, 9, 11, 13];
    let mut selected = Vec::new();
    let mut size = 0;
    let mut cursors = [0; WINDOWS];
    loop {
        let mut advanced = false;
        for index in ORDER {
            let Some(term) = windows[index].get(cursors[index]) else {
                continue;
            };
            cursors[index] += 1;
            advanced = true;
            let needed = term.text.len() + usize::from(!selected.is_empty());
            if size + needed <= MAX_QUERY_BYTES && selected.len() < MAX_TERMS {
                selected.push(term.text.as_str());
                size += needed;
            }
        }
        if !advanced || selected.len() >= MAX_TERMS {
            break;
        }
    }
    Ok(selected.join(" "))
}

/// Stateless host entry point. Reading is bounded even when stdin is untrusted.
pub fn run(args: &[String]) -> Result<String> {
    let precise = match args {
        [] => None,
        [flag, value] if flag == "--query" => Some(value.as_str()),
        _ => return Err("Use aienv query [--query SINGLE_LINE] < task.txt".into()),
    };
    let mut task = Vec::new();
    std::io::stdin()
        .take((MAX_TASK_BYTES + 1) as u64)
        .read_to_end(&mut task)?;
    let task = std::str::from_utf8(&task)?;
    let query = project(task, precise)?;
    json(
        &serde_json::json!({"has_terms":has_terms(&query),"query":query,"method":if precise.is_some() {"explicit"} else {"whole-task-lexical"}}),
    )
}

/// Match brief's minimum literal term length, including multibyte Unicode terms.
pub fn has_terms(query: &str) -> bool {
    query
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .any(|term| term.len() >= 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_contract_signal_outranks_incidental_wrapper_path_words() {
        let query = NavigationQuery::new(
            "environment grammar parser tests preserve RunOperation run_operation",
        );
        let noise = query.evidence(
            "environment/grammar/parser.rs",
            "// preserve tests\nfn parse(): {}\n",
        );
        let owner = query.evidence(
            "src/operations.rs",
            "pub fn run_operation() {}\nstruct RunOperation;\n",
        );
        assert!(noise.literal_score > owner.literal_score);
        assert!(owner.rank() > noise.rank());
        assert_eq!(owner.specificity, (false, 2));
        assert_eq!(
            query
                .terms()
                .iter()
                .filter(|word| word.as_str() == "run_operation")
                .count(),
            1
        );
    }

    #[test]
    fn focus_uses_whole_identifier_and_late_declaration_shaped_source() {
        let query = NavigationQuery::new("environment RunOperation run_operation");
        let source = format!(
            "// run_operation\n{}pub fn run_operation() {{}}\nstruct RunOperation;\n",
            "// environment run_operation_extra RunOperationSuffix\n".repeat(80)
        );
        let evidence = query.evidence("src/change.rs", &source);
        assert_eq!(evidence.specificity.1, 2);
        assert_eq!(evidence.focus_lines[0], 82);
        assert!(evidence.focus_lines.contains(&83));
        let prefix_only = query.evidence(
            "src/other.rs",
            "fn run_operation_extra() {}\nstruct RunOperationSuffix;\n",
        );
        assert_eq!(prefix_only.specificity.1, 0);
        assert!(prefix_only.focus_lines.is_empty());
        let other_case = query.evidence(
            "src/other.rs",
            "struct RUNOPERATION; fn RUN_OPERATION() {}\n",
        );
        assert_eq!(other_case.specificity.1, 0);
        assert!(other_case.literal_score > 0);
    }

    #[test]
    fn unicode_identifiers_and_focus_limits_remain_deterministic() {
        let query = NavigationQuery::new("ЛимитРезерва Квота_Резерва");
        let valid = query.evidence(
            "src/лимит.rs",
            &"struct ЛимитРезерва; // Квота_Резерва\n".repeat(40),
        );
        assert_eq!(valid.specificity.1, 2);
        assert_eq!(valid.focus_lines, (1..=16).collect::<Vec<_>>());
        let prefixes = query.evidence("src/лимит.rs", "ЛимитРезерваДругой Квота_Резерва_extra");
        assert_eq!(prefixes.specificity.1, 0);
    }

    #[test]
    fn explicit_path_is_exact_canonical_signal_without_scope_expansion() {
        let query = NavigationQuery::new("Change `src/target.rs`, not ./src/alias.rs or ../src/outside.rs or /src/absolute.rs or src//repeat.rs");
        let chosen = query.evidence("src/target.rs", "");
        assert!(chosen.specificity.0);
        let hidden = NavigationQuery::new("Change `.internal/target.rs`.");
        assert!(hidden.evidence(".internal/target.rs", "").specificity.0);
        assert!(!hidden.evidence("internal/target.rs", "").specificity.0);
        for path in [
            "target.rs",
            "src/target.rs.backup",
            "other/src/target.rs",
            "src/alias.rs",
            "src/outside.rs",
            "src/absolute.rs",
            "src/repeat.rs",
        ] {
            assert!(!query.evidence(path, "").specificity.0, "{path}");
        }
        assert!(
            chosen.rank()
                > query
                    .evidence("another.rs", "struct StrongContract;")
                    .rank()
        );
        assert!(query.evidence("src/target.rs", "").focus_lines.is_empty());
    }

    #[test]
    fn ordinary_words_preserve_original_literal_fallback() {
        let query = NavigationQuery::new("update stock retention");
        let a = query.evidence("src/stock.rs", "retention\n");
        let b = query.evidence("src/other.rs", "update stock retention\n");
        assert_eq!(a.specificity, (false, 0));
        assert_eq!(b.specificity, (false, 0));
        assert_eq!(a.literal_score, 110);
        assert_eq!(b.literal_score, 30);
        assert!(a.rank() > b.rank());
        assert!(a.focus_lines.is_empty());
    }

    #[test]
    fn short_tasks_and_explicit_queries_keep_their_words() {
        assert_eq!(
            project("  Update\nquote and\tstock_ceiling.  ", None).unwrap(),
            "Update quote and stock_ceiling."
        );
        assert_eq!(
            project("ignored", Some("  Quote::total  src/pricing.rs  ")).unwrap(),
            "Quote::total  src/pricing.rs"
        );
        for query in ["", " \n ", "quote\nstock", "quote\0stock"] {
            assert!(project("task", Some(query)).is_err());
        }
        assert!(project("task", Some(&"я".repeat(1025))).is_err());
        assert_eq!(
            project("task", Some(&"я".repeat(1024))).unwrap().len(),
            MAX_QUERY_BYTES
        );
        assert!(!has_terms(&project("a", None).unwrap()));
        assert!(has_terms(&project("я", None).unwrap()));
    }

    #[test]
    fn long_tasks_include_late_identifiers_and_distributed_requirements() {
        let task = format!("Preserve EarlyContract. {} Keep midpoint_ownership. {} Finally enforce stock_ceiling and check_regressions with КвотаРезерва; reject mutation.", "ordinary background context ".repeat(160), "ordinary background context ".repeat(160));
        let query = project(&task, None).unwrap();
        for word in [
            "EarlyContract",
            "midpoint_ownership",
            "stock_ceiling",
            "check_regressions",
            "КвотаРезерва",
            "mutation",
        ] {
            assert!(query.contains(word), "missing {word}: {query}");
        }
        assert!(query.len() <= MAX_QUERY_BYTES);
        assert_eq!(query, project(&task, None).unwrap());
    }

    #[test]
    fn dense_unicode_and_many_symbols_are_bounded_without_fragments() {
        let task = (0..600)
            .map(|n| format!("Граница_{n} правило_{n} "))
            .collect::<String>();
        let query = project(&task, None).unwrap();
        assert!(query.len() <= MAX_QUERY_BYTES);
        assert!(query.split_whitespace().count() <= MAX_TERMS);
        assert!(query.contains("Граница_599"));
        for term in query.split_whitespace() {
            assert!(task.split_whitespace().any(|original| original == term));
        }
        for invalid in ["".to_owned(), "x".repeat(65537), "task\0symbol".into()] {
            assert!(project(&invalid, None).is_err());
        }
    }
}
