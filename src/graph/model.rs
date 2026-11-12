use crate::{
    project::{normalize_path, sha, Project, Snapshot},
    Result,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEMA: u32 = 1;
pub const LANGUAGES: &[&str] = &[
    "c",
    "cpp",
    "csharp",
    "rust",
    "python",
    "typescript",
    "javascript",
];
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub flags: Vec<String>,
    pub compile_commands: Option<String>,
    pub libclang: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub defines: Vec<String>,
    #[serde(default)]
    pub project: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub language: String,
    pub options: Options,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Analyzer {
    pub name: String,
    pub version: String,
    pub capabilities: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub signature: String,
    pub definition: bool,
    pub external: bool,
    #[serde(default)]
    pub implicit: bool,
    pub path: Option<String>,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub kind: String,
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub dispatch: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: String,
    pub path: Option<String>,
    pub line: usize,
    pub message: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Unresolved {
    pub source: Option<String>,
    pub kind: String,
    pub expression: String,
    pub path: String,
    pub line: usize,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Input {
    pub path: String,
    pub hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Graph {
    pub schema_version: u32,
    pub language: String,
    pub analyzer: Analyzer,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub diagnostics: Vec<Diagnostic>,
    pub unresolved: Vec<Unresolved>,
    pub inputs: Vec<Input>,
    pub analysis_complete: bool,
    pub limitations: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub schema_version: u32,
    pub project_root: String,
    pub spec: Spec,
    pub source_key: String,
    // Relative indexed inputs bind source-only refresh eligibility to the
    // original Spec. Older caches remain readable, but cannot prove that only
    // source (rather than project configuration) changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hashes: Option<BTreeMap<String, String>>,
    pub backend_hash: String,
    pub index_hash: String,
    pub graph: Graph,
}

pub fn language(value: &str) -> Result<&str> {
    if LANGUAGES.contains(&value) {
        Ok(value)
    } else {
        Err(format!(
            "Semantic adapters available: {}. Other languages retain explicit lexical context.",
            LANGUAGES.join(", ")
        )
        .into())
    }
}
pub fn relevant(language: &str, path: &str) -> bool {
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match language {
        "c" | "cpp" => matches!(
            ext,
            "c" | "cc"
                | "cpp"
                | "cxx"
                | "c++"
                | "C"
                | "h"
                | "hh"
                | "hpp"
                | "hxx"
                | "h++"
                | "ipp"
                | "inl"
                | "inc"
        ),
        "csharp" => ext == "cs",
        "rust" => {
            ext == "rs"
                || matches!(
                    name,
                    "Cargo.toml"
                        | "Cargo.lock"
                        | "rust-project.json"
                        | "rust-toolchain"
                        | "rust-toolchain.toml"
                )
                || matches!(path, ".cargo/config" | ".cargo/config.toml")
        }
        "python" => {
            matches!(ext, "py" | "pyi")
                || matches!(
                    name,
                    "pyproject.toml"
                        | "setup.cfg"
                        | "poetry.lock"
                        | "uv.lock"
                        | "Pipfile"
                        | "Pipfile.lock"
                        | "pyrightconfig.json"
                        | "py.typed"
                )
                || (name.starts_with("requirements") && ext == "txt")
        }
        "typescript" | "javascript" => {
            matches!(
                ext,
                "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" | "json"
            ) || matches!(name, "yarn.lock" | "pnpm-lock.yaml")
        }
        _ => false,
    }
}
pub fn source_key(snapshot: &Snapshot, spec: &Spec) -> Result<String> {
    source_key_from_hashes(
        spec,
        snapshot
            .files
            .iter()
            .filter(|(path, _)| relevant(&spec.language, path))
            .map(|(path, entry)| (path.as_str(), entry.hash.as_str())),
    )
}
pub(crate) fn source_key_from_hashes<'a>(
    spec: &Spec,
    hashes: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<String> {
    let mut parts = serde_json::to_vec(spec)?;
    for (path, hash) in hashes {
        parts.extend_from_slice(path.as_bytes());
        parts.push(0);
        parts.extend_from_slice(hash.as_bytes());
        parts.push(0);
    }
    Ok(sha(parts))
}
/// Configuration and lockfiles participate in freshness, but changing them
/// requires an explicit refresh rather than an opportunistic source refresh.
pub(crate) fn source_file(language: &str, path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match language {
        "c" | "cpp" | "csharp" => relevant(language, path),
        "rust" => ext == "rs",
        "python" => matches!(ext, "py" | "pyi"),
        "typescript" | "javascript" => {
            matches!(
                ext,
                "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs"
            )
        }
        _ => false,
    }
}
pub fn public_id(language: &str, id: &str) -> String {
    format!("{language}:{}", &sha(id)[..16])
}
pub fn graph_hash(graph: &Graph) -> Result<String> {
    Ok(sha(serde_json::to_vec(graph)?))
}
pub fn validate(
    project: &Project,
    snapshot: &Snapshot,
    graph: &Graph,
    requested: &str,
) -> Result<()> {
    if graph.schema_version != SCHEMA || graph.language != requested {
        return Err("Analyzer returned a different schema/language".into());
    }
    if graph.analyzer.name.trim().is_empty() || graph.analyzer.version.trim().is_empty() {
        return Err("Missing compiler identity".into());
    }
    if graph.analyzer.version == "unavailable" {
        let explanation = graph
            .diagnostics
            .iter()
            .take(5)
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!(
            "Semantic analyzer unavailable; no graph published. {}",
            explanation.chars().take(3000).collect::<String>()
        )
        .into());
    }
    if graph.nodes.len() > 100_000 || graph.edges.len() > 500_000 || graph.inputs.len() > 20_000 {
        return Err("Semantic graph exceeds supported size; narrow selected files".into());
    }
    let mut ids = BTreeSet::new();
    let mut aliases = BTreeSet::new();
    for node in &graph.nodes {
        if node.id.is_empty()
            || node.id.len() > 8192
            || node.id.chars().any(char::is_control)
            || !ids.insert(node.id.as_str())
        {
            return Err("Duplicate or invalid compiler symbol ID".into());
        }
        if !aliases.insert(public_id(requested, &node.id)) {
            return Err("Compact symbol ID collision; use a narrower index".into());
        }
        if !matches!(
            node.kind.as_str(),
            "function" | "type" | "field" | "alias" | "variable" | "namespace"
        ) {
            return Err(format!("Unsupported symbol kind: {}", node.kind).into());
        }
        if !node.external && node.path.is_none() {
            if !node.implicit
                || node.line != 0
                || node.column != 0
                || node.end_line != 0
                || node.start_byte != 0
                || node.end_byte != 0
            {
                return Err(
                    "Project symbol without source must be explicitly compiler-implicit".into(),
                );
            }
            continue;
        }
        if !node.external {
            let path = node
                .path
                .as_ref()
                .ok_or("Project declaration has no source path")?;
            if normalize_path(path)? != *path {
                return Err("Noncanonical project declaration path".into());
            }
            let entry = snapshot
                .files
                .get(path)
                .ok_or_else(|| format!("Analyzer declaration outside indexed sources: {path}"))?;
            project.resolve(path)?;
            if node.start_byte > node.end_byte
                || node.end_byte > entry.text.len()
                || !entry.text.is_char_boundary(node.start_byte)
                || !entry.text.is_char_boundary(node.end_byte)
            {
                return Err(
                    "Analyzer declaration does not have exact UTF-8 source boundaries".into(),
                );
            }
            if node.line == 0 || node.column == 0 || node.end_line < node.line {
                return Err("Invalid source coordinates".into());
            }
        }
    }
    for edge in &graph.edges {
        if !ids.contains(edge.source.as_str()) || !ids.contains(edge.target.as_str()) {
            return Err("Graph edge endpoint has no compiler declaration".into());
        }
        if !matches!(
            edge.kind.as_str(),
            "calls" | "references" | "uses_type" | "inherits" | "aliases" | "contains"
        ) || !matches!(edge.dispatch.as_str(), "static" | "virtual" | "dynamic")
        {
            return Err("Unknown semantic relation or dispatch".into());
        }
        if !snapshot.files.contains_key(&edge.path) {
            return Err(format!("Graph edge outside indexed source: {}", edge.path).into());
        }
    }
    for unresolved in &graph.unresolved {
        if unresolved
            .source
            .as_deref()
            .is_some_and(|id| !ids.contains(id))
        {
            return Err("Unresolved relation refers to unknown source symbol".into());
        }
    }
    if graph.analysis_complete
        && graph
            .diagnostics
            .iter()
            .any(|d| matches!(d.severity.as_str(), "error" | "fatal"))
    {
        return Err("Compiler errors cannot be reported as complete analysis".into());
    }
    Ok(())
}
