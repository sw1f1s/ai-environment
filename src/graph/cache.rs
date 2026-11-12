use super::{model::*, process};
use crate::{
    project::{normalize_path, sha, Project, Snapshot},
    Result,
};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn within(deadline: Option<Instant>) -> Result<()> {
    if deadline.is_some_and(|value| Instant::now() >= value) {
        return Err("On-demand refresh deadline exhausted".into());
    }
    Ok(())
}

pub fn key(language: &str) -> Result<String> {
    super::model::language(language)?;
    Ok(format!("semantic/{language}.json"))
}
fn file_hash_until(path: &Path, deadline: Option<Instant>) -> Result<String> {
    within(deadline)?;
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > 512 * 1024 * 1024 {
        return Err(format!(
            "Invalid or oversized semantic dependency: {}",
            path.display()
        )
        .into());
    }
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut block = [0; 65536];
    let mut bytes = 0;
    loop {
        within(deadline)?;
        let count = file.read(&mut block)?;
        if count == 0 {
            break;
        }
        bytes += count;
        if bytes > 512 * 1024 * 1024 {
            return Err("Semantic dependency grew beyond file limit".into());
        }
        hash.update(&block[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn bundle() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("AI_ENV_ANALYZERS") {
        let path = PathBuf::from(path).canonicalize()?;
        if !path.is_dir() {
            return Err("AI_ENV_ANALYZERS must be a directory".into());
        }
        return Ok(path);
    }
    let exe = std::env::current_exe()?;
    if let Some(parent) = exe.parent().and_then(Path::parent) {
        let path = parent.join("analyzers");
        if path.is_dir() {
            return Ok(path);
        }
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/ai-env/scripts/analyzers");
    if source.is_dir() {
        return Ok(source);
    }
    Err("Semantic adapters are not packaged. Build/install the complete AI Env skill or set AI_ENV_ANALYZERS.".into())
}
fn backend_until(language: &str, deadline: Option<Instant>) -> Result<(PathBuf, String)> {
    within(deadline)?;
    let directory = bundle()?;
    let names: &[&str] = match language {
        "c" | "cpp" => &["clang.py"],
        "csharp" => &["csharp.py"],
        "rust" => &["rust.py", "rust_lsp.py"],
        "python" => &["python.py"],
        "typescript" | "javascript" => &["typescript.py", "typescript.cjs"],
        _ => return Err("Unsupported semantic backend".into()),
    };
    let script = directory.join(names[0]);
    if !script.is_file() {
        return Err(format!("Analyzer unavailable for {language}: {}", script.display()).into());
    }
    let mut identity = file_hash_until(&script, deadline)?.into_bytes();
    let runtime_variables: &[&str] = match language {
        "rust" => &["AI_ENV_RUST_ANALYZER", "AI_ENV_RUST_SYSROOT"],
        "typescript" | "javascript" => &["AI_ENV_NODE"],
        _ => &[],
    };
    for variable in runtime_variables {
        identity.extend_from_slice(variable.as_bytes());
        identity.push(0);
        if let Some(value) = std::env::var_os(variable) {
            identity.extend_from_slice(value.as_encoded_bytes());
            let path = Path::new(&value);
            if path.is_file() {
                identity.extend_from_slice(file_hash_until(path, deadline)?.as_bytes());
            }
        }
        identity.push(0);
    }
    if matches!(language, "rust" | "typescript" | "javascript") {
        let profile = directory
            .parent()
            .ok_or("Invalid analyzer bundle")?
            .join("bin/runtimes.json");
        if profile.is_file() {
            identity.extend_from_slice(file_hash_until(&profile, deadline)?.as_bytes());
        }
    }
    for name in &names[1..] {
        identity.extend_from_slice(name.as_bytes());
        identity.extend_from_slice(file_hash_until(&directory.join(name), deadline)?.as_bytes());
    }
    let vendor = directory
        .parent()
        .ok_or("Invalid analyzer bundle")?
        .join("vendor");
    match language {
        "python" => {
            fingerprint_tree(&vendor.join("jedi"), &mut identity, deadline)?;
            fingerprint_tree(&vendor.join("parso"), &mut identity, deadline)?;
        }
        "typescript" | "javascript" => {
            fingerprint_tree(&vendor.join("typescript"), &mut identity, deadline)?
        }
        _ => {}
    }
    if language == "csharp" {
        let dlls = directory
            .parent()
            .ok_or("Invalid analyzer bundle")?
            .join("bin/roslyn");
        let mut paths = Vec::new();
        if let Ok(entries) = fs::read_dir(&dlls) {
            for entry in entries {
                let path = entry?.path();
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        paths.sort();
        if !paths
            .iter()
            .any(|p| p.file_name().is_some_and(|n| n == "GraphAnalyzer.dll"))
        {
            return Err("Roslyn helper unavailable. Run python3 scripts/dev.py build with .NET SDK installed.".into());
        }
        for path in paths {
            identity.extend_from_slice(path.file_name().unwrap().as_encoded_bytes());
            identity.extend_from_slice(file_hash_until(&path, deadline)?.as_bytes());
        }
    }
    Ok((script, sha(identity)))
}
fn fingerprint_tree(root: &Path, identity: &mut Vec<u8>, deadline: Option<Instant>) -> Result<()> {
    fn visit(path: &Path, files: &mut Vec<PathBuf>, deadline: Option<Instant>) -> Result<()> {
        within(deadline)?;
        if !path.is_dir() || path.is_symlink() {
            return Err(format!(
                "Missing or linked analyzer dependency: {}. Build/install the complete skill.",
                path.display()
            )
            .into());
        }
        for entry in fs::read_dir(path)? {
            within(deadline)?;
            let entry = entry?;
            let path = entry.path();
            if entry.file_name() == "__pycache__" || path.extension().is_some_and(|s| s == "pyc") {
                continue;
            }
            if path.is_symlink() {
                return Err("Linked analyzer dependency is unsupported".into());
            }
            if path.is_dir() {
                visit(&path, files, deadline)?;
            } else if path.is_file() {
                if files.len() >= 20000 {
                    return Err("Analyzer dependency tree exceeds file limit".into());
                }
                files.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    visit(root, &mut files, deadline)?;
    files.sort();
    if files.is_empty() {
        return Err("Empty analyzer dependency tree".into());
    }
    for path in files {
        identity.extend_from_slice(path.strip_prefix(root)?.as_os_str().as_encoded_bytes());
        identity.push(0);
        identity.extend_from_slice(file_hash_until(&path, deadline)?.as_bytes());
    }
    Ok(())
}
fn explicit_inputs(spec: &Spec, deadline: Option<Instant>) -> Result<Vec<Input>> {
    let mut paths = Vec::new();
    if let Some(path) = &spec.options.compile_commands {
        paths.push(path);
    }
    if let Some(path) = &spec.options.libclang {
        paths.push(path);
    }
    if let Some(path) = &spec.options.project {
        paths.push(path);
    }
    paths.extend(spec.options.references.iter());
    let mut result = Vec::new();
    for path in paths {
        result.push(Input {
            path: path.clone(),
            hash: file_hash_until(Path::new(path), deadline)?,
        });
    }
    Ok(result)
}
pub fn read(project: &Project, language: &str) -> Result<Envelope> {
    let cached: Option<Envelope> = project.read_state(&key(language)?)?;
    let cached = cached.ok_or_else(|| {
        format!("No semantic index for {language}. Run graph build --language {language}.")
    })?;
    if cached.schema_version != SCHEMA
        || cached.spec.language != language
        || cached.graph.schema_version != SCHEMA
        || cached.graph.language != language
    {
        return Err("Unsupported or inconsistent semantic cache. Rebuild graph.".into());
    }
    if graph_hash(&cached.graph)? != cached.index_hash {
        return Err("Semantic cache checksum mismatch. Rebuild graph.".into());
    }
    if let Some(hashes) = &cached.source_hashes {
        for (path, hash) in hashes {
            if normalize_path(path)? != *path
                || !relevant(language, path)
                || hash.len() != 64
                || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err("Invalid semantic source manifest. Rebuild graph.".into());
            }
        }
        if source_key_from_hashes(
            &cached.spec,
            hashes.iter().map(|(p, h)| (p.as_str(), h.as_str())),
        )? != cached.source_key
        {
            return Err(
                "Semantic source manifest/options checksum mismatch. Rebuild graph.".into(),
            );
        }
    }
    Ok(cached)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refresh_allowance_shares_deadline_and_caps_attempts() {
        let mut budget = RefreshBudget::new();
        let first = budget.claim().unwrap();
        assert_eq!(first, budget.claim().unwrap());
        assert!(budget
            .claim()
            .unwrap_err()
            .to_string()
            .contains("attempt limit"));
        let mut expired = RefreshBudget {
            deadline: Instant::now(),
            attempts: 0,
        };
        assert!(expired
            .claim()
            .unwrap_err()
            .to_string()
            .contains("deadline"));
        assert_eq!(expired.attempts, 0);
    }
}
pub fn freshness(project: &Project, snapshot: &Snapshot, cached: &Envelope) -> Result<()> {
    freshness_until(project, snapshot, cached, None)
}
fn freshness_until(
    project: &Project,
    snapshot: &Snapshot,
    cached: &Envelope,
    deadline: Option<Instant>,
) -> Result<()> {
    within(deadline)?;
    if cached.project_root != project.root.to_string_lossy() {
        return Err("Semantic index belongs to a different project root".into());
    }
    if source_key(snapshot, &cached.spec)? != cached.source_key {
        return Err("Semantic sources/options changed".into());
    }
    if backend_until(&cached.spec.language, deadline)?.1 != cached.backend_hash {
        return Err("Analyzer implementation changed".into());
    }
    for input in &cached.graph.inputs {
        if file_hash_until(Path::new(&input.path), deadline)? != input.hash {
            return Err(format!("Semantic dependency changed: {}", input.path).into());
        }
    }
    Ok(())
}
pub fn load_current(project: &Project, language: &str) -> Result<Envelope> {
    let snapshot = project.snapshot()?;
    load_at(project, &snapshot, language)
}
pub fn load_at(project: &Project, snapshot: &Snapshot, language: &str) -> Result<Envelope> {
    let cached = read(project, language)?;
    freshness(project, snapshot, &cached).map_err(|e| {
        format!("Stale semantic index: {e}. Run graph refresh --language {language}.")
    })?;
    validate(project, snapshot, &cached.graph, language)?;
    Ok(cached)
}

/// One brief shares this allowance across all already configured languages.
/// Hashing checks the same deadline between reads; helper cleanup is separately
/// bounded by process::invoke_until. No discovery or installation happens here.
pub(crate) struct RefreshBudget {
    deadline: Instant,
    attempts: usize,
}
impl RefreshBudget {
    pub(crate) fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(20),
            attempts: 0,
        }
    }
    fn claim(&mut self) -> Result<Instant> {
        within(Some(self.deadline))?;
        if self.attempts >= 2 {
            return Err("On-demand refresh attempt limit (2) reached".into());
        }
        self.attempts += 1;
        Ok(self.deadline)
    }
}

fn source_only_change(
    project: &Project,
    snapshot: &Snapshot,
    cached: &Envelope,
    deadline: Instant,
) -> Result<()> {
    within(Some(deadline))?;
    if cached.project_root != project.root.to_string_lossy() {
        return Err("Semantic index belongs to a different project root".into());
    }
    let hashes = cached
        .source_hashes
        .as_ref()
        .ok_or("Old cache has no source manifest; explicit refresh required")?;
    // Check provenance even when the source key is stale: source drift must
    // never hide a toolchain, configuration or external dependency change.
    if backend_until(&cached.spec.language, Some(deadline))?.1 != cached.backend_hash {
        return Err("Analyzer implementation/runtime changed".into());
    }
    let current: BTreeMap<_, _> = snapshot
        .files
        .iter()
        .filter(|(path, _)| relevant(&cached.spec.language, path))
        .map(|(path, entry)| (path.clone(), entry.hash.clone()))
        .collect();
    for path in hashes.keys().chain(current.keys()) {
        if !source_file(&cached.spec.language, path) && hashes.get(path) != current.get(path) {
            return Err(format!("Semantic configuration changed: {path}").into());
        }
    }
    // Explicit inputs stay strict even if an unusual source-looking file was
    // passed as --project/--reference. They are not implicitly reconfigured.
    for input in explicit_inputs(&cached.spec, Some(deadline))? {
        if !cached
            .graph
            .inputs
            .iter()
            .any(|old| old.path == input.path && old.hash == input.hash)
        {
            return Err(format!("Explicit semantic input changed: {}", input.path).into());
        }
    }
    for input in &cached.graph.inputs {
        within(Some(deadline))?;
        let relative = Path::new(&input.path)
            .strip_prefix(&project.root)
            .ok()
            .and_then(Path::to_str);
        let indexed_source = relative.is_some_and(|path| {
            source_file(&cached.spec.language, path)
                && hashes.get(path) == Some(&input.hash)
                && normalize_path(path).is_ok_and(|normalized| normalized == path)
        });
        if !indexed_source && file_hash_until(Path::new(&input.path), Some(deadline))? != input.hash
        {
            return Err(format!("Semantic dependency changed: {}", input.path).into());
        }
    }
    Ok(())
}

/// Returns only fresh validated facts. A stale source manifest authorizes
/// rebuilding with its recorded Spec, never reading old relationships as facts.
pub(crate) fn load_for_brief(
    project: &Project,
    snapshot: &Snapshot,
    language: &str,
    allowance: &mut RefreshBudget,
) -> Result<(Envelope, bool)> {
    within(Some(allowance.deadline))?;
    let cached = read(project, language)?;
    if source_key(snapshot, &cached.spec)? == cached.source_key {
        freshness_until(project, snapshot, &cached, Some(allowance.deadline))?;
        validate(project, snapshot, &cached.graph, language)?;
        return Ok((cached, false));
    }
    source_only_change(project, snapshot, &cached, allowance.deadline)?;
    let deadline = allowance.claim()?;
    build_at(project, cached.spec, snapshot, Some(deadline))
        .map(|refreshed| (refreshed, true))
        .map_err(|e| format!("On-demand refresh failed: {e}").into())
}

pub fn build(project: &Project, spec: Spec) -> Result<Envelope> {
    let snapshot = project.snapshot()?;
    build_at(project, spec, &snapshot, None)
}
fn build_at(
    project: &Project,
    spec: Spec,
    snapshot: &Snapshot,
    deadline: Option<Instant>,
) -> Result<Envelope> {
    within(deadline)?;
    super::model::language(&spec.language)?;
    let source = source_key(snapshot, &spec)?;
    let (script, backend_hash) = backend_until(&spec.language, deadline)?;
    let inputs_before = explicit_inputs(&spec, deadline)?;
    let files: Vec<_> = snapshot
        .files
        .values()
        .filter(|f| relevant(&spec.language, &f.path))
        .collect();
    if files.is_empty() {
        return Err(format!("No indexed {} source files", spec.language).into());
    }
    for path in &spec.options.files {
        if !files.iter().any(|f| f.path == *path) {
            return Err(format!("Selected semantic file is absent or not indexed: {path}").into());
        }
    }
    let request = serde_json::json!({"schema_version":SCHEMA,"language":spec.language,"root":project.root,"files":files,"options":spec.options});
    let payload = serde_json::to_vec(&request)?;
    let bytes = if let Some(deadline) = deadline {
        process::invoke_until(&script, payload, &project.root, deadline)?
    } else {
        process::invoke(&script, payload, &project.root)?
    };
    within(deadline)?;
    let mut graph: Graph = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Invalid semantic analyzer response: {e}"))?;
    graph.nodes.sort_by(|a, b| a.id.cmp(&b.id));
    graph.edges.sort_by(|a, b| {
        (&a.source, &a.kind, &a.target, &a.path, a.line, a.column)
            .cmp(&(&b.source, &b.kind, &b.target, &b.path, b.line, b.column))
    });
    graph.edges.dedup_by(|a, b| {
        a.source == b.source
            && a.target == b.target
            && a.kind == b.kind
            && a.path == b.path
            && a.line == b.line
            && a.column == b.column
            && a.dispatch == b.dispatch
    });
    for input in inputs_before {
        if !graph
            .inputs
            .iter()
            .any(|i| i.path == input.path && i.hash == input.hash)
        {
            graph.inputs.push(input);
        }
    }
    graph.inputs.sort_by(|a, b| a.path.cmp(&b.path));
    graph
        .inputs
        .dedup_by(|a, b| a.path == b.path && a.hash == b.hash);
    validate(project, snapshot, &graph, &spec.language)?;
    let source_hashes = snapshot
        .files
        .iter()
        .filter(|(path, _)| relevant(&spec.language, path))
        .map(|(path, entry)| (path.clone(), entry.hash.clone()))
        .collect();
    let cached = Envelope {
        schema_version: SCHEMA,
        project_root: project.root.to_string_lossy().into_owned(),
        spec,
        source_key: source,
        source_hashes: Some(source_hashes),
        backend_hash,
        index_hash: graph_hash(&graph)?,
        graph,
    };
    let after = project.snapshot()?;
    if deadline.is_some() && after.hash != snapshot.hash {
        return Err("Project changed during on-demand analysis; no graph published".into());
    }
    freshness_until(project, &after, &cached, deadline)
        .map_err(|e| format!("Semantic inputs changed during analysis; no graph published: {e}"))?;
    within(deadline)?;
    project.write_state(&key(&cached.spec.language)?, &cached)?;
    Ok(cached)
}
