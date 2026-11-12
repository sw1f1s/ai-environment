//! Bind native model plans to a bounded catalog captured at dispatch.
//!
//! Models select read references; they never author controller read scopes or
//! verification paths. The private manifest must be persisted with the original
//! dispatch inputs. Its checksum detects corruption/mixups, not a hostile writer
//! who can replace host state. Workflow retains live filesystem, ownership,
//! approved-check, proposal and snapshot/CAS authorization.
use crate::{
    project::{normalize_path, scope_contains, sha, valid_id, Snapshot, MAX_FILE},
    Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const SCHEMA: u32 = 1;
const MAX_FILES: usize = 20_000;
const MAX_BASE_BYTES: usize = 8 * 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 128 * 1024;
const MAX_SCOPES: usize = 100_000;
const MAX_REFS: usize = 128;
const MAX_PUBLIC_BYTES: usize = 16 * 1024;
const MAX_ANSWER_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    id: String,
    path: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Base {
    schema: u32,
    snapshot: String,
    files: BTreeMap<String, String>,
    refs: Vec<Reference>,
    omitted_scopes: usize,
    indexed_omitted: usize,
    source_digest: String,
    digest: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Task {
    id: String,
    title: String,
    instructions: String,
    paths: Vec<String>,
    read_refs: Vec<String>,
    depends: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Obligation {
    id: String,
    acceptance: Vec<usize>,
    tasks: Vec<String>,
    risk: String,
    checks: Vec<String>,
}

fn hash_valid(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn canonical(path: &str) -> Result<()> {
    if normalize_path(path)? != path {
        return Err("Plan paths must be canonical repository-relative paths".into());
    }
    Ok(())
}
fn scope_valid(path: &str) -> Result<()> {
    if path.len() > 512 || path.contains(['*', '?', '[', ']']) {
        return Err("Plan scopes must be exact paths/prefixes up to 512 UTF-8 bytes".into());
    }
    if path != "." {
        canonical(path)?;
    }
    Ok(())
}
fn text_valid(text: &str, limit: usize) -> bool {
    !text.trim().is_empty() && !text.contains('\0') && text.len() <= limit
}
fn unique<T: Ord>(items: &[T]) -> bool {
    items.iter().collect::<BTreeSet<_>>().len() == items.len()
}
fn reference_id(path: &str) -> String {
    format!("r{}", &sha(path.as_bytes())[..10])
}
fn checksum(base: &Base) -> Result<String> {
    let mut bytes = b"ai-env-model-plan-base-1\0".to_vec();
    bytes.extend(serde_json::to_vec(&(
        base.schema,
        &base.snapshot,
        &base.files,
        &base.refs,
        base.omitted_scopes,
        base.indexed_omitted,
        &base.source_digest,
    ))?);
    Ok(sha(bytes))
}

// Borrow prefixes rather than copying every ancestor of every indexed path.
// The explicit candidate cap also bounds deeply nested adversarial inputs.
fn scopes(files: &BTreeMap<String, String>) -> Result<BTreeSet<&str>> {
    let mut result = BTreeSet::new();
    for path in files.keys() {
        result.insert(path.as_str());
        for (index, _) in path.match_indices('/') {
            let parent = &path[..index];
            if files.contains_key(parent) {
                return Err("Indexed plan file cannot be an ancestor of another file".into());
            }
            result.insert(parent);
        }
        if result.len() > MAX_SCOPES {
            return Err("Plan catalog exceeds 100000 candidate scopes".into());
        }
    }
    Ok(result)
}
fn public_value(base: &Base) -> Value {
    json!({"refs":base.refs,"omitted_scopes":base.omitted_scopes,"indexed_omitted":base.indexed_omitted})
}
fn validate(base: &Base, entry_snapshot: &str) -> Result<()> {
    if base.schema != SCHEMA
        || !hash_valid(&base.snapshot)
        || base.snapshot != entry_snapshot
        || !hash_valid(entry_snapshot)
        || !hash_valid(&base.source_digest)
        || !hash_valid(&base.digest)
        || base.files.len() > MAX_FILES
        || base.refs.len() > MAX_REFS
        || checksum(base)? != base.digest
    {
        return Err(
            "Invalid, corrupt or mismatched model plan base; retain dispatch inputs".into(),
        );
    }
    for (path, hash) in &base.files {
        canonical(path)?;
        if !hash_valid(hash) {
            return Err("Invalid indexed file hash in model plan base".into());
        }
    }
    let candidates = scopes(&base.files)?;
    if base.omitted_scopes != candidates.len().saturating_sub(base.refs.len()) {
        return Err("Corrupt model plan catalog omission count".into());
    }
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for reference in &base.refs {
        scope_valid(&reference.path)?;
        if reference.path == "."
            || !candidates.contains(reference.path.as_str())
            || reference.id != reference_id(&reference.path)
            || !ids.insert(&reference.id)
            || !paths.insert(&reference.path)
        {
            return Err("Invalid, duplicate or colliding model plan reference".into());
        }
    }
    if serde_json::to_vec(&public_value(base))?.len() > MAX_PUBLIC_BYTES {
        return Err("Public plan catalog exceeds 16 KiB".into());
    }
    Ok(())
}
fn read_base(base: &Value) -> Result<Base> {
    if serde_json::to_vec(base)?.len() > MAX_BASE_BYTES {
        return Err("Model plan base exceeds 8 MiB".into());
    }
    serde_json::from_value(base.clone())
        .map_err(|error| format!("Missing or invalid private model plan base: {error}").into())
}

fn visible_files<'a>(source: &str, files: &'a BTreeMap<String, String>) -> Vec<&'a str> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for line in source.lines() {
        let Some(header) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
            continue;
        };
        // Full and excerpt are the actual brief header formats. A source line
        // merely mentioning a path does not promote that file into visibility.
        let candidate = header
            .rfind(" excerpt")
            .map(|end| &header[..end])
            .or_else(|| {
                let end = header.rfind(":1..")?;
                let rest = &header[end + 4..];
                let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                (digits > 0 && (digits == rest.len() || rest[digits..].starts_with(" hash=")))
                    .then_some(&header[..end])
            });
        if let Some((path, _)) = candidate.and_then(|path| files.get_key_value(path)) {
            if seen.insert(path.as_str()) {
                result.push(path.as_str());
            }
        }
    }
    result
}

/// Capture once before dispatch. Only indexed files and their actual ancestor
/// directories can become read references; root is never added implicitly.
/// `source` is the dispatched brief, or empty when no source packet is present.
pub fn capture(snapshot: &Snapshot, source: &str) -> Result<Value> {
    if !hash_valid(&snapshot.hash)
        || snapshot.files.len() > MAX_FILES
        || source.len() > MAX_SOURCE_BYTES
    {
        return Err("Invalid or oversized model plan dispatch input".into());
    }
    if !source.is_empty()
        && source.lines().next() != Some(format!("snapshot {}", snapshot.hash).as_str())
    {
        return Err("Model plan source packet must match the dispatch snapshot".into());
    }
    let mut files = BTreeMap::new();
    for (path, file) in &snapshot.files {
        canonical(path)?;
        if file.path != *path
            || file.text.len() > MAX_FILE as usize
            || file.text.contains('\0')
            || !hash_valid(&file.hash)
            || sha(file.text.as_bytes()) != file.hash
        {
            return Err("Inconsistent indexed file in model plan dispatch snapshot".into());
        }
        files.insert(path.clone(), file.hash.clone());
    }
    let candidates = scopes(&files)?;
    let visible = visible_files(source, &files);
    let mut ranked = visible.clone();
    for path in &visible {
        for (index, _) in path.rmatch_indices('/') {
            ranked.push(&path[..index]);
        }
    }
    ranked.extend(files.keys().map(String::as_str));
    ranked.extend(candidates.iter().copied());
    let mut base = Base {
        schema: SCHEMA,
        snapshot: snapshot.hash.clone(),
        files: files.clone(),
        refs: vec![],
        omitted_scopes: candidates.len(),
        indexed_omitted: snapshot.omitted,
        source_digest: sha(source.as_bytes()),
        digest: String::new(),
    };
    let mut seen = BTreeSet::new();
    let mut ids = BTreeMap::new();
    for path in ranked {
        if !seen.insert(path) || scope_valid(path).is_err() {
            continue;
        }
        let id = reference_id(path);
        if ids.insert(id.clone(), path).is_some() {
            return Err("Model plan reference ID collision; no catalog was dispatched".into());
        }
        if base.refs.len() == MAX_REFS {
            continue;
        }
        base.refs.push(Reference {
            id,
            path: path.into(),
        });
        base.omitted_scopes = candidates.len() - base.refs.len();
        if serde_json::to_vec(&public_value(&base))?.len() > MAX_PUBLIC_BYTES {
            base.refs.pop();
            base.omitted_scopes = candidates.len() - base.refs.len();
        }
    }
    base.digest = checksum(&base)?;
    validate(&base, &snapshot.hash)?;
    let encoded = serde_json::to_vec(&base)?;
    if encoded.len() > MAX_BASE_BYTES {
        return Err("Model plan base exceeds 8 MiB".into());
    }
    Ok(serde_json::from_slice(&encoded)?)
}

/// Return only compact model-visible references and explicit omission counts.
/// No file hashes, source copies or private manifest checksum enter the prompt.
pub fn public(base: &Value) -> Result<Value> {
    let base = read_base(base)?;
    validate(&base, &base.snapshot)?;
    Ok(public_value(&base))
}

fn minimize(scopes: impl IntoIterator<Item = String>) -> Vec<String> {
    let scopes = scopes.into_iter().collect::<BTreeSet<_>>();
    scopes
        .iter()
        .filter(|path| {
            !scopes
                .iter()
                .any(|other| other != *path && scope_contains(other, path))
        })
        .cloned()
        .collect()
}
fn ancestor(tasks: &[Task], reader: usize, writer: usize, seen: &mut BTreeSet<usize>) -> bool {
    tasks[reader].depends.iter().any(|id| {
        let index = tasks.iter().position(|task| &task.id == id).unwrap();
        index == writer || (seen.insert(index) && ancestor(tasks, index, writer, seen))
    })
}
fn overlaps(left: &[String], right: &[String]) -> bool {
    left.iter().any(|left| {
        right
            .iter()
            .any(|right| scope_contains(left, right) || scope_contains(right, left))
    })
}

/// Translate strict native plan fields to the existing workflow contract.
/// The original model answer is unchanged. This function never reads live
/// source or re-captures a catalog; replay uses the durable dispatch base.
pub fn bind(answer: &Value, base: &Value, entry_snapshot: &str) -> Result<Value> {
    let base = read_base(base)?;
    validate(&base, entry_snapshot)?;
    if serde_json::to_vec(answer)?.len() > MAX_ANSWER_BYTES
        || answer.get("snapshot").and_then(Value::as_str) != Some(entry_snapshot)
    {
        return Err("Model plan answer must fit 16 MiB and match the dispatched snapshot".into());
    }
    let tasks: Vec<Task> = serde_json::from_value(answer["tasks"].clone()).map_err(|error| {
        format!("Native tasks require id/title/instructions/paths/read_refs/depends, never context: {error}")
    })?;
    let obligations: Vec<Obligation> = serde_json::from_value(answer["verification"].clone())
        .map_err(|error| {
            format!("Native obligations omit paths; Rust derives them from task scopes: {error}")
        })?;
    let acceptance: Vec<String> = serde_json::from_value(answer["acceptance"].clone())?;
    let checks: Vec<String> = serde_json::from_value(answer["checks"].clone())?;
    if tasks.is_empty()
        || tasks.len() > 64
        || acceptance.is_empty()
        || acceptance.len() > 64
        || obligations.is_empty()
        || obligations.len() > 6
        || acceptance.iter().any(|text| !text_valid(text, 4096))
        || checks.len() > 64
        || !unique(&checks)
        || checks.iter().any(|id| !valid_id(id))
    {
        return Err(
            "Native plan needs 1..64 tasks/criteria, 1..6 obligations and unique check names"
                .into(),
        );
    }
    let references = base
        .refs
        .iter()
        .map(|reference| (reference.id.as_str(), reference.path.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut ids = BTreeSet::new();
    for task in &tasks {
        if !valid_id(&task.id)
            || !ids.insert(task.id.as_str())
            || !text_valid(&task.title, 1000)
            || !text_valid(&task.instructions, 16384)
            || task.paths.is_empty()
            || task.paths.len() > 64
            || !unique(&task.paths)
            || task.read_refs.len() > 64
            || !unique(&task.read_refs)
            || task.depends.len() > 64
            || !unique(&task.depends)
            || task
                .read_refs
                .iter()
                .any(|id| !references.contains_key(id.as_str()))
        {
            return Err("Invalid/duplicate native task or unknown read reference".into());
        }
        for path in &task.paths {
            scope_valid(path)?;
            for (index, _) in path.match_indices('/') {
                if base.files.contains_key(&path[..index]) {
                    return Err("Task write scope has an indexed file as an ancestor".into());
                }
            }
        }
    }
    for task in &tasks {
        if task
            .depends
            .iter()
            .any(|id| id == &task.id || !ids.contains(id.as_str()))
        {
            return Err("Unknown or self native task dependency".into());
        }
    }
    let mut done = BTreeSet::new();
    loop {
        let before = done.len();
        for task in &tasks {
            if task.depends.iter().all(|id| done.contains(id.as_str())) {
                done.insert(task.id.as_str());
            }
        }
        if done.len() == tasks.len() {
            break;
        }
        if done.len() == before {
            return Err("Native task dependency cycle".into());
        }
    }
    let mut contexts = Vec::new();
    for task in &tasks {
        let mut context = task
            .read_refs
            .iter()
            .map(|id| references[id.as_str()].to_owned())
            .collect::<Vec<_>>();
        for dependency in &task.depends {
            context.extend(
                tasks
                    .iter()
                    .find(|task| &task.id == dependency)
                    .unwrap()
                    .paths
                    .clone(),
            );
        }
        let context = minimize(context);
        if context.len() > 64 {
            return Err("Derived task context exceeds 64 scopes".into());
        }
        contexts.push(context);
    }
    for (reader, task) in tasks.iter().enumerate() {
        for (writer, provider) in tasks.iter().enumerate() {
            if reader == writer {
                continue;
            }
            let preceding = ancestor(&tasks, reader, writer, &mut BTreeSet::new());
            if overlaps(&contexts[reader], &provider.paths) && !preceding {
                return Err(format!(
                    "Task {} reads task {} writes; declare its provider dependency",
                    task.id, provider.id
                )
                .into());
            }
            if overlaps(&task.paths, &provider.paths)
                && !preceding
                && !ancestor(&tasks, writer, reader, &mut BTreeSet::new())
            {
                return Err(
                    "Overlapping task writes require an explicit provider dependency".into(),
                );
            }
        }
    }
    let mut obligation_ids = BTreeSet::new();
    let mut covered = BTreeSet::new();
    let mut derived = Vec::new();
    for obligation in &obligations {
        if !valid_id(&obligation.id)
            || !obligation_ids.insert(&obligation.id)
            || obligation.acceptance.is_empty()
            || obligation.acceptance.len() > 64
            || !unique(&obligation.acceptance)
            || obligation
                .acceptance
                .iter()
                .any(|index| *index >= acceptance.len())
            || obligation.tasks.is_empty()
            || obligation.tasks.len() > 64
            || !unique(&obligation.tasks)
            || obligation.tasks.iter().any(|id| !ids.contains(id.as_str()))
            || obligation.risk.trim().is_empty()
            || obligation.risk.contains('\0')
            || obligation.risk.chars().count() > 768
            || obligation.checks.len() > 64
            || !unique(&obligation.checks)
            || obligation.checks.iter().any(|name| !checks.contains(name))
            || (!checks.is_empty() && obligation.checks.is_empty())
        {
            return Err(
                "Invalid native obligation IDs, owners, acceptance, risk or check names".into(),
            );
        }
        let mut paths = Vec::new();
        for id in &obligation.tasks {
            let index = tasks.iter().position(|task| &task.id == id).unwrap();
            paths.extend(tasks[index].paths.clone());
            paths.extend(contexts[index].clone());
        }
        let paths = minimize(paths);
        if paths.is_empty() || paths.len() > 16 {
            return Err("Derived obligation paths exceed 16 scopes; narrow task scopes or split obligations explicitly".into());
        }
        covered.extend(obligation.acceptance.iter().copied());
        derived.push(paths);
    }
    if covered.len() != acceptance.len() {
        return Err("Native verification obligations must cover every acceptance criterion".into());
    }
    let mut bound = answer.clone();
    for (index, context) in contexts.into_iter().enumerate() {
        let object = bound["tasks"][index].as_object_mut().unwrap();
        object.remove("read_refs");
        object.insert("context".into(), json!(context));
    }
    for (index, paths) in derived.into_iter().enumerate() {
        bound["verification"][index]["paths"] = json!(paths);
    }
    Ok(bound)
}
