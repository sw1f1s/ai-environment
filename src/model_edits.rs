//! Bind model-authored text edits to the immutable dispatch snapshot.
//!
//! The private manifest belongs in durable host inputs, never in the prompt.
//! Its checksum detects corruption/mixups; it is not a signature or a security
//! boundary against a writer who controls host state. Workflow still authorizes
//! scopes, resolves replacements and performs the actual snapshot/file CAS.
use crate::{
    project::{normalize_path, sha, Snapshot, MAX_FILE},
    Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const SCHEMA: u32 = 1;
const MAX_FILES: usize = 20_000;
const MAX_BASE_BYTES: usize = 8 * 1024 * 1024;
const MAX_EDITS: usize = 128;
const MAX_EDIT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Base {
    schema: u32,
    snapshot: String,
    files: BTreeMap<String, String>,
    digest: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ModelEdit {
    Replacements(Replacements),
    Source(Source),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Replacements {
    path: String,
    replacements: Vec<Replacement>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Replacement {
    old: String,
    new: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    path: String,
    source: String,
}

fn hash_valid(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn canonical(path: &str) -> Result<()> {
    if normalize_path(path)? != path {
        return Err("Model edits require canonical repository-relative paths".into());
    }
    Ok(())
}
fn digest(base: &Base) -> Result<String> {
    let mut bytes = b"ai-env-model-edit-base-1\0".to_vec();
    bytes.extend(serde_json::to_vec(&(
        base.schema,
        &base.snapshot,
        &base.files,
    ))?);
    Ok(sha(bytes))
}
fn validate(base: &Base, entry_snapshot: &str) -> Result<()> {
    if base.schema != SCHEMA
        || !hash_valid(&base.snapshot)
        || !hash_valid(entry_snapshot)
        || base.snapshot != entry_snapshot
        || !hash_valid(&base.digest)
        || base.files.len() > MAX_FILES
    {
        return Err(
            "Invalid or mismatched model edit base; retain the original dispatch inputs".into(),
        );
    }
    for (path, hash) in &base.files {
        canonical(path)?;
        if !hash_valid(hash) {
            return Err("Invalid file hash in model edit base".into());
        }
    }
    if digest(base)? != base.digest {
        return Err("Corrupt model edit base checksum; retain the original dispatch inputs".into());
    }
    Ok(())
}

/// Capture all indexed original hashes before dispatch. Persist this value in
/// private inputs once; never reconstruct it from current source during replay.
pub fn capture(snapshot: &Snapshot) -> Result<Value> {
    let mut files = BTreeMap::new();
    if snapshot.files.len() > MAX_FILES {
        return Err("Model edit base exceeds 20000 indexed files".into());
    }
    for (path, file) in &snapshot.files {
        canonical(path)?;
        if file.path != *path
            || file.text.len() > MAX_FILE as usize
            || file.text.contains('\0')
            || !hash_valid(&file.hash)
            || sha(file.text.as_bytes()) != file.hash
        {
            return Err("Inconsistent indexed file in dispatch snapshot".into());
        }
        files.insert(path.clone(), file.hash.clone());
    }
    let mut base = Base {
        schema: SCHEMA,
        snapshot: snapshot.hash.clone(),
        files,
        digest: String::new(),
    };
    base.digest = digest(&base)?;
    validate(&base, &snapshot.hash)?;
    let encoded = serde_json::to_vec(&base)?;
    if encoded.len() > MAX_BASE_BYTES {
        return Err("Model edit base exceeds the 8 MiB private manifest limit".into());
    }
    Ok(serde_json::from_slice(&encoded)?)
}

fn text_valid(text: &str, nonempty: bool) -> Result<()> {
    if (nonempty && text.is_empty()) || text.len() > MAX_FILE as usize || text.contains('\0') {
        return Err(
            "Model edit text must fit 2 MiB UTF-8 without NUL; old must be nonempty".into(),
        );
    }
    Ok(())
}

/// Add only the ORIGINAL expected_hash to strict model-only edit objects.
/// No current-source reads are performed here. A newly appearing destination
/// therefore remains expected_hash=null, and the existing CAS rejects it.
/// Replacements are deliberately not resolved here: workflow owns uniqueness,
/// sequential virtual contents, scope checks and final materialized size.
pub fn bind(edits: &Value, base: &Value, entry_snapshot: &str) -> Result<Value> {
    if serde_json::to_vec(base)?.len() > MAX_BASE_BYTES {
        return Err("Model edit base exceeds the 8 MiB private manifest limit".into());
    }
    let base: Base = serde_json::from_value(base.clone())
        .map_err(|error| format!("Missing or invalid private model edit base: {error}"))?;
    validate(&base, entry_snapshot)?;
    let rows = edits.as_array().ok_or("Model edits must be an array")?;
    if rows.len() > MAX_EDITS || serde_json::to_vec(edits)?.len() > MAX_EDIT_BYTES {
        return Err("Model edits exceed 128 files or the 16 MiB edit input limit".into());
    }
    let mut seen = BTreeSet::new();
    let mut bound = Vec::with_capacity(rows.len());
    for row in rows {
        if row
            .as_object()
            .is_some_and(|object| object.contains_key("expected_hash"))
        {
            return Err(
                "Model edits must omit expected_hash; Rust binds the original dispatch hash".into(),
            );
        }
        let edit: ModelEdit = serde_json::from_value(row.clone()).map_err(|error| {
            format!("Use only {{path,source}} or {{path,replacements:[{{old,new}}]}}: {error}")
        })?;
        let path = match &edit {
            ModelEdit::Replacements(edit) => &edit.path,
            ModelEdit::Source(edit) => &edit.path,
        };
        canonical(path)?;
        if !seen.insert(path.clone()) {
            return Err("Duplicate model file edit; group replacements under one path".into());
        }
        let expected = base.files.get(path);
        match &edit {
            ModelEdit::Source(edit) => text_valid(&edit.source, false)?,
            ModelEdit::Replacements(edit) => {
                if expected.is_none() {
                    return Err("New model files require source, not replacements".into());
                }
                if edit.replacements.is_empty() || edit.replacements.len() > 128 {
                    return Err("A model file edit requires 1..128 replacements".into());
                }
                for replacement in &edit.replacements {
                    text_valid(&replacement.old, true)?;
                    text_valid(&replacement.new, false)?;
                }
            }
        }
        let mut row = row.clone();
        row.as_object_mut()
            .ok_or("Model edit must be an object")?
            .insert("expected_hash".into(), json!(expected));
        bound.push(row);
    }
    Ok(Value::Array(bound))
}
