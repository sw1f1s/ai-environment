//! Selector-only correction of an otherwise saved model response.
//!
//! No source, filesystem, proof, or semantic decision is consulted or changed.
//! The host must persist/bind the original response and catalog and run all
//! normal plan/QA gates again after merging. This is not a relaxed acceptance
//! gate or an authenticity boundary for a writer controlling host artifacts.
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_SPEC: usize = 32 * 1024;
const MAX_REFS: usize = 256;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Row {
    id: String,
    selected: Vec<Value>,
    selector_field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    omitted_title_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    omitted_instruction_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    evidence: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    kind: String,
    role: String,
    snapshot: String,
    owner_ids: Vec<String>,
    rows: Vec<Row>,
    choices: BTreeMap<String, Vec<String>>,
    refs: Vec<Value>,
}

fn fail(message: &str) -> Box<dyn std::error::Error + Send + Sync> {
    format!("Metadata repair unavailable or invalid: {message}").into()
}
fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn text(value: &str, max: usize, nonempty: bool) -> bool {
    (!nonempty || !value.trim().is_empty()) && !value.contains('\0') && value.len() <= max
}
fn strings(value: &Value, max: usize) -> Option<Vec<String>> {
    let values = value.as_array()?;
    if values.len() > max {
        return None;
    }
    values
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect()
}
fn unique<T: Ord>(values: &[T]) -> bool {
    values.iter().collect::<BTreeSet<_>>().len() == values.len()
}
fn canonical(path: &str, allow_root: bool) -> bool {
    if allow_root && path == "." {
        return true;
    }
    text(path, 512, true)
        && !path.contains('\\')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
fn excerpt(value: &str, max: usize) -> (String, usize) {
    let mut length = value.len().min(max);
    while !value.is_char_boundary(length) {
        length -= 1;
    }
    (value[..length].into(), value.len() - length)
}
fn only(value: &Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.keys().all(|key| keys.contains(&key.as_str())))
}

struct Catalog {
    refs: Vec<Value>,
    choices: BTreeMap<String, Vec<String>>,
    all: Vec<String>,
}
fn catalog(public: &Value, qa: bool) -> Result<Catalog> {
    let input = public["refs"]
        .as_array()
        .ok_or_else(|| fail("public catalog needs refs"))?;
    if input.len() > MAX_REFS {
        return Err(fail("reference catalog exceeds 256 entries"));
    }
    let owners = if qa {
        let map = public["by_obligation"]
            .as_object()
            .ok_or_else(|| fail("QA catalog needs by_obligation"))?;
        if map.is_empty() || map.len() > 6 || map.keys().any(|key| !id(key)) {
            return Err(fail("invalid QA obligation catalog"));
        }
        map.keys().cloned().collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };
    let mut refs = Vec::new();
    let mut all = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in input {
        let name = raw["id"]
            .as_str()
            .ok_or_else(|| fail("reference needs ID"))?;
        let path = raw["path"]
            .as_str()
            .ok_or_else(|| fail("reference needs path"))?;
        if !id(name) || !seen.insert(name.to_owned()) || !canonical(path, !qa) {
            return Err(fail("duplicate/invalid reference ID or path"));
        }
        if qa {
            let line = raw["line"]
                .as_u64()
                .ok_or_else(|| fail("source line missing"))?;
            let end = raw["end_line"]
                .as_u64()
                .ok_or_else(|| fail("source end_line missing"))?;
            let obligations = strings(&raw["obligations"], 6)
                .ok_or_else(|| fail("source obligations missing"))?;
            if line == 0
                || end < line
                || obligations.is_empty()
                || !unique(&obligations)
                || obligations.iter().any(|o| !owners.contains(o))
            {
                return Err(fail("invalid source range or obligation membership"));
            }
            refs.push(
                json!({"id":name,"path":path,"line":line,"end_line":end,"obligations":obligations}),
            );
        } else {
            // Whitelist public fields: never copy source/private hashes into a
            // correction prompt even if an upstream caller supplies extras.
            refs.push(json!({"id":name,"path":path}));
        }
        all.push(name.to_owned());
    }
    let mut choices = BTreeMap::new();
    for owner in owners {
        let supplied = strings(&public["by_obligation"][&owner], MAX_REFS)
            .ok_or_else(|| fail("invalid per-obligation choices"))?;
        let derived = refs
            .iter()
            .filter(|r| r["obligations"].as_array().unwrap().contains(&json!(owner)))
            .map(|r| r["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        if !unique(&supplied)
            || supplied.iter().collect::<BTreeSet<_>>() != derived.iter().collect()
        {
            return Err(fail("catalog choices disagree with reference ownership"));
        }
        let mut supplied = supplied;
        supplied.sort();
        choices.insert(owner, supplied);
    }
    all.sort();
    Ok(Catalog { refs, choices, all })
}

fn plan_rows(original: &Value) -> Option<&Vec<Value>> {
    let rows = original["tasks"].as_array()?;
    if rows.is_empty() || rows.len() > 64 {
        return None;
    }
    let mut names = BTreeSet::new();
    for row in rows {
        if !only(
            row,
            &[
                "id",
                "title",
                "instructions",
                "paths",
                "depends",
                "read_refs",
                "context",
            ],
        ) || !id(row["id"].as_str()?)
            || !names.insert(row["id"].as_str()?)
            || !text(row["title"].as_str()?, 1000, true)
            || !text(row["instructions"].as_str()?, 16384, true)
        {
            return None;
        }
        let paths = strings(&row["paths"], 64)?;
        if paths.is_empty() || !unique(&paths) || paths.iter().any(|p| !canonical(p, true)) {
            return None;
        }
    }
    for row in rows {
        let own = row["id"].as_str()?;
        let depends = strings(&row["depends"], 64)?;
        if !unique(&depends)
            || depends
                .iter()
                .any(|d| !names.contains(d.as_str()) || d == own)
        {
            return None;
        }
    }
    // Unknown/self/cyclic ownership is a plan problem, never a selector repair.
    let mut ready = BTreeSet::new();
    loop {
        let previous = ready.len();
        for row in rows {
            if row["depends"]
                .as_array()?
                .iter()
                .all(|d| ready.contains(d.as_str().unwrap()))
            {
                ready.insert(row["id"].as_str()?);
            }
        }
        if ready.len() == rows.len() {
            break;
        }
        if ready.len() == previous {
            return None;
        }
    }
    Some(rows)
}

/// Offer correction only for reference fields, preserving the original meaning.
/// None means no selector error or a response requiring normal semantic repair.
/// Explicit errors denote malformed/oversized correction inputs, not permission
/// to dispatch a larger correction. The host's normal gates remain authoritative.
pub fn prepare(role: &str, original: &Value, public_catalog: &Value) -> Result<Option<Value>> {
    let qa = role == "qa";
    if !qa && !matches!(role, "coordinator" | "architect" | "planner") {
        return Ok(None);
    }
    let Some(snapshot) = original["snapshot"].as_str().filter(|s| text(s, 128, true)) else {
        return Ok(None);
    };
    if public_catalog
        .get("snapshot")
        .is_some_and(|s| s.as_str() != Some(snapshot))
    {
        return Ok(None);
    }
    let cards = catalog(public_catalog, qa)?;
    let source = if qa {
        let Some(rows) = original["verification"].as_array() else {
            return Ok(None);
        };
        if rows.is_empty() || rows.len() > 6 {
            return Ok(None);
        }
        let Some(proofs) = strings(&original["proofs"], 64) else {
            return Ok(None);
        };
        if !unique(&proofs) || proofs.iter().any(|p| !id(p)) {
            return Ok(None);
        }
        if !matches!(original["verdict"].as_str(), Some("accept" | "changes")) {
            return Ok(None);
        }
        let ids = rows
            .iter()
            .filter_map(|r| r["id"].as_str())
            .collect::<BTreeSet<_>>();
        if ids.len() != rows.len() || ids != cards.choices.keys().map(String::as_str).collect() {
            return Ok(None);
        }
        rows
    } else {
        let Some(rows) = plan_rows(original) else {
            return Ok(None);
        };
        rows
    };
    let owners = source
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let mut rows = Vec::new();
    let mut choices = BTreeMap::new();
    for raw in source {
        let name = raw["id"].as_str().unwrap();
        let available = if qa { &cards.choices[name] } else { &cards.all };
        let field = if qa { "source_refs" } else { "read_refs" };
        let legacy = if qa { "locations" } else { "context" };
        let maximum = if qa { 8 } else { 64 };
        if raw
            .get(field)
            .and_then(Value::as_array)
            .is_some_and(|r| r.len() > maximum)
            || raw
                .get(legacy)
                .and_then(Value::as_array)
                .is_some_and(|r| r.len() > maximum)
        {
            return Err(fail("saved selector count exceeds the small-repair bound"));
        }
        let selected = if let Some(value) = raw.get(field) {
            let Some(values) = strings(value, maximum) else {
                return Ok(None);
            };
            values.into_iter().map(Value::String).collect::<Vec<_>>()
        } else if let Some(value) = raw.get(legacy) {
            let Some(values) = value.as_array() else {
                return Ok(None);
            };
            if qa {
                if values.iter().any(|v| {
                    !only(v, &["path", "line"])
                        || v["path"].as_str().is_none()
                        || v["line"].as_u64().is_none()
                }) {
                    return Ok(None);
                }
                values.clone()
            } else {
                let Some(values) = strings(value, maximum) else {
                    return Ok(None);
                };
                values.into_iter().map(Value::String).collect()
            }
        } else {
            Vec::new()
        };
        if selected
            .iter()
            .any(|v| serde_json::to_vec(v).map_or(true, |v| v.len() > 1024))
        {
            return Err(fail(
                "saved selector description exceeds the small-repair bound",
            ));
        }
        let mut row = Row {
            id: name.into(),
            selected,
            selector_field: if raw.get(field).is_some() {
                field.into()
            } else if raw.get(legacy).is_some() {
                legacy.into()
            } else {
                "missing".into()
            },
            title: None,
            instructions: None,
            omitted_title_bytes: None,
            omitted_instruction_bytes: None,
            status: None,
            evidence: None,
        };
        if qa {
            if !only(
                raw,
                &[
                    "id",
                    "status",
                    "proofs",
                    "source_refs",
                    "locations",
                    "evidence",
                ],
            ) {
                return Ok(None);
            }
            let Some(status) = raw["status"].as_str() else {
                return Ok(None);
            };
            let Some(evidence) = raw["evidence"]
                .as_str()
                .filter(|s| text(s, 3072, true) && s.chars().count() <= 768)
            else {
                return Ok(None);
            };
            let Some(proofs) = strings(&raw["proofs"], 64) else {
                return Ok(None);
            };
            let top = strings(&original["proofs"], 64).unwrap();
            if !matches!(status, "verified" | "gap" | "inspected")
                || !unique(&proofs)
                || proofs.iter().any(|p| !id(p) || !top.contains(p))
                || (status == "gap" && original["verdict"] != "changes")
                || (status == "inspected" && (!proofs.is_empty() || !top.is_empty()))
                || (status == "verified" && proofs.is_empty())
            {
                return Ok(None);
            }
            if status != "gap" && available.is_empty() {
                return Ok(None);
            }
            row.status = Some(status.into());
            row.evidence = Some(evidence.into());
        } else {
            let (title, omitted_title) = excerpt(raw["title"].as_str().unwrap(), 160);
            let (instructions, omitted_instructions) =
                excerpt(raw["instructions"].as_str().unwrap(), 512);
            row.title = Some(title);
            row.instructions = Some(instructions);
            row.omitted_title_bytes = Some(omitted_title);
            row.omitted_instruction_bytes = Some(omitted_instructions);
        }
        let supplied = strings(&raw[field], maximum);
        let bad = raw.get(legacy).is_some()
            || supplied.as_ref().is_none_or(|v| {
                !unique(v)
                    || v.iter().any(|r| !available.contains(r))
                    || (qa && row.status.as_deref() != Some("gap") && v.is_empty())
            });
        if bad {
            choices.insert(name.into(), available.clone());
            rows.push(row);
        }
    }
    if rows.is_empty() {
        return Ok(None);
    }
    let spec = serde_json::to_value(Spec {
        kind: if qa {
            "qa_sources".into()
        } else {
            "plan_reads".into()
        },
        role: role.into(),
        snapshot: snapshot.into(),
        owner_ids: owners,
        rows,
        choices,
        refs: cards.refs,
    })?;
    checked(&spec)?;
    Ok(Some(spec))
}

fn checked(value: &Value) -> Result<Spec> {
    if serde_json::to_vec(value)?.len() > MAX_SPEC {
        return Err(fail("selector specification exceeds 32 KiB"));
    }
    let spec: Spec = serde_json::from_value(value.clone())
        .map_err(|e| fail(&format!("invalid selector specification: {e}")))?;
    let qa = spec.kind == "qa_sources";
    if (!qa && spec.kind != "plan_reads")
        || (qa && spec.role != "qa")
        || (!qa && !matches!(spec.role.as_str(), "coordinator" | "architect" | "planner"))
        || !text(&spec.snapshot, 128, true)
        || spec.rows.is_empty()
        || spec.owner_ids.is_empty()
        || spec.owner_ids.len() > if qa { 6 } else { 64 }
        || !unique(&spec.owner_ids)
        || spec.owner_ids.iter().any(|n| !id(n))
        || spec.rows.len() > spec.owner_ids.len()
    {
        return Err(fail("invalid kind, role, snapshot or row count"));
    }
    let by = spec
        .owner_ids
        .iter()
        .map(|owner| {
            let values = spec
                .refs
                .iter()
                .filter(|r| {
                    r["obligations"]
                        .as_array()
                        .is_some_and(|o| o.contains(&json!(owner)))
                })
                .map(|r| r["id"].clone())
                .collect::<Vec<_>>();
            (owner.clone(), json!(values))
        })
        .collect::<serde_json::Map<_, _>>();
    let cards = catalog(&json!({"refs":spec.refs,"by_obligation":by}), qa)?;
    if cards.refs != spec.refs {
        return Err(fail("specification contains non-public reference fields"));
    }
    let mut seen = BTreeSet::new();
    for row in &spec.rows {
        if !spec.owner_ids.contains(&row.id)
            || !seen.insert(row.id.clone())
            || row.selected.len() > if qa { 8 } else { 64 }
            || !(if qa {
                matches!(
                    row.selector_field.as_str(),
                    "source_refs" | "locations" | "missing"
                )
            } else {
                matches!(
                    row.selector_field.as_str(),
                    "read_refs" | "context" | "missing"
                )
            })
            || row.selected.iter().any(|v| {
                serde_json::to_vec(v).map_or(true, |v| v.len() > 1024)
                    || if row.selector_field == "locations" {
                        !only(v, &["path", "line"])
                            || v["path"].as_str().is_none()
                            || v["line"].as_u64().is_none()
                    } else {
                        !v.is_string()
                    }
            })
            || (row.selector_field == "missing" && !row.selected.is_empty())
            || (qa
                && (row.title.is_some()
                    || row.instructions.is_some()
                    || row.omitted_title_bytes.is_some()
                    || row.omitted_instruction_bytes.is_some()
                    || !matches!(
                        row.status.as_deref(),
                        Some("verified" | "gap" | "inspected")
                    )
                    || row
                        .evidence
                        .as_ref()
                        .is_none_or(|s| !text(s, 3072, true) || s.chars().count() > 768)))
            || (!qa
                && (row.status.is_some()
                    || row.evidence.is_some()
                    || row.title.as_ref().is_none_or(|s| !text(s, 160, true))
                    || row
                        .instructions
                        .as_ref()
                        .is_none_or(|s| !text(s, 512, true))
                    || row.omitted_title_bytes.is_none()
                    || row.omitted_instruction_bytes.is_none()))
        {
            return Err(fail("invalid or duplicate correction owner/meaning row"));
        }
        let actual = spec
            .choices
            .get(&row.id)
            .ok_or_else(|| fail("missing owner choices"))?;
        let expected = if qa {
            &cards.choices[&row.id]
        } else {
            &cards.all
        };
        if !unique(actual)
            || actual.iter().collect::<BTreeSet<_>>() != expected.iter().collect()
            || (qa && row.status.as_deref() != Some("gap") && actual.is_empty())
        {
            return Err(fail("wrong-owner or corrupt selector choices"));
        }
    }
    if spec.choices.len() != spec.rows.len() {
        return Err(fail("unexpected selector choice owner"));
    }
    Ok(spec)
}

/// Strict row-specific choices. Merge additionally enforces uniqueness and the
/// required nonempty refs for a resolved verified/inspected QA row.
pub fn schema(value: &Value) -> Result<Value> {
    let spec = checked(value)?;
    let qa = spec.kind == "qa_sources";
    let variants=spec.rows.iter().map(|row| {
        let choices=&spec.choices[&row.id];
        let items=if choices.is_empty() {json!({"type":"string"})} else {json!({"type":"string","enum":choices})};
        json!({"type":"object","additionalProperties":false,"required":["id","refs","unresolved"],"properties":{
            "id":{"type":"string","enum":[row.id]},"refs":{"type":"array","minItems":0,"maxItems":if choices.is_empty(){0}else if qa{8}else{64},"items":items},
            "unresolved":{"type":"boolean"}}})
    }).collect::<Vec<_>>();
    Ok(
        json!({"type":"object","additionalProperties":false,"required":["snapshot","selections"],"properties":{
        "snapshot":{"type":"string","enum":[spec.snapshot]},
        "selections":{"type":"array","minItems":spec.rows.len(),"maxItems":spec.rows.len(),"items":{"anyOf":variants}}}}),
    )
}

pub fn prompt(value: &Value, error: &str) -> Result<String> {
    let spec = checked(value)?;
    let (error, omitted) = excerpt(error, 1024);
    Ok(format!("AI Env role: {}. Repair selector metadata only. No tools, source reads/writes, coding, new agents or reassessment. Keep the SAVED task meaning or QA status/evidence unchanged. Choose only listed refs supporting that saved meaning; do not guess, add proof claims or turn a gap into acceptance. If the references cannot support it, set unresolved=true. Unresolved triggers normal full assessment; it is not success. Return only the structured selections. Excerpt omission counts are explicit; do not assume omitted instructions.\n\n{}\n",spec.role,
        serde_json::to_string(&json!({"goal":"Repair reference fields only","snapshot":spec.snapshot,"metadata_repair":value,"error":error,"error_omitted_bytes":omitted}))?))
}

/// Merge into a clone of the original. Only rejected selector fields change;
/// ownership, dependencies, edits, evidence, proofs and verdict stay untouched.
pub fn merge(original: &Value, patch: &Value, value: &Value) -> Result<Value> {
    let spec = checked(value)?;
    if !only(patch, &["snapshot", "selections"])
        || patch.as_object().is_none_or(|m| m.len() != 2)
        || patch["snapshot"].as_str() != Some(&spec.snapshot)
        || original["snapshot"].as_str() != Some(&spec.snapshot)
    {
        return Err(fail("patch/original snapshot or top-level shape mismatch"));
    }
    let qa = spec.kind == "qa_sources";
    let by = spec
        .owner_ids
        .iter()
        .map(|owner| {
            (
                owner.clone(),
                json!(spec
                    .refs
                    .iter()
                    .filter(|r| r["obligations"]
                        .as_array()
                        .is_some_and(|ids| ids.contains(&json!(owner))))
                    .map(|r| r["id"].clone())
                    .collect::<Vec<_>>()),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let expected = prepare(
        &spec.role,
        original,
        &json!({"refs":spec.refs,"by_obligation":by}),
    )?
    .ok_or_else(|| fail("original response no longer matches a selector-only repair"))?;
    if expected != *value {
        return Err(fail(
            "specification does not match the saved original owners/meaning",
        ));
    }
    let selections = patch["selections"]
        .as_array()
        .ok_or_else(|| fail("selections must be an array"))?;
    if selections.len() != spec.rows.len() {
        return Err(fail("selection count differs from rejected row count"));
    }
    let mut seen = BTreeSet::new();
    let mut replacements = BTreeMap::new();
    for selection in selections {
        if !only(selection, &["id", "refs", "unresolved"])
            || selection.as_object().is_none_or(|m| m.len() != 3)
        {
            return Err(fail("unexpected selection fields"));
        }
        let owner = selection["id"]
            .as_str()
            .ok_or_else(|| fail("selection owner missing"))?;
        let row = spec
            .rows
            .iter()
            .find(|r| r.id == owner)
            .ok_or_else(|| fail("unknown selection owner"))?;
        if !seen.insert(owner.to_owned()) {
            return Err(fail("duplicate selection owner"));
        }
        let refs = strings(&selection["refs"], if qa { 8 } else { 64 })
            .ok_or_else(|| fail("invalid selector list or count"))?;
        if !unique(&refs) || refs.iter().any(|r| !spec.choices[owner].contains(r)) {
            return Err(fail("unknown, duplicate or wrong-owner selector"));
        }
        let unresolved = selection["unresolved"]
            .as_bool()
            .ok_or_else(|| fail("unresolved must be boolean"))?;
        if unresolved {
            return Err("Metadata repair cannot resolve saved references".into());
        }
        if qa && row.status.as_deref() != Some("gap") && refs.is_empty() {
            return Err(fail("resolved verified/inspected result needs source refs"));
        }
        replacements.insert(owner.to_owned(), refs);
    }
    let mut output = original.clone();
    let rows = output[if qa { "verification" } else { "tasks" }]
        .as_array_mut()
        .ok_or_else(|| fail("saved rows missing"))?;
    for row in rows {
        if let Some(refs) = replacements.get(row["id"].as_str().unwrap()) {
            let row = row.as_object_mut().unwrap();
            row.remove(if qa { "locations" } else { "context" });
            row.insert(
                if qa {
                    "source_refs".into()
                } else {
                    "read_refs".into()
                },
                json!(refs),
            );
        }
    }
    Ok(output)
}
