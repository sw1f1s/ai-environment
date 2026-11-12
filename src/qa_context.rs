//! Change-focused QA navigation. Discovery counts and selected source are not
//! execution evidence or proof that a verification obligation is covered.
use crate::{
    brief, profile,
    project::{normalize_path, scope_contains, Project},
    retrieval,
    verification::Obligation,
    Result,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_REQUESTED_PATHS: usize = 1024;
const MAX_REQUESTED_BYTES: usize = 65536;
const MAX_SCOPES: usize = 64;
const MAX_SCOPE_BYTES: usize = 2048;

#[derive(Deserialize)]
struct Task {
    id: String,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    context: Vec<String>,
}
fn array<T: serde::de::DeserializeOwned>(state: &Value, key: &str) -> Result<Vec<T>> {
    match state.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(value) => Ok(serde_json::from_value(value.clone())?),
    }
}
fn canonical(path: &str) -> Result<()> {
    if path != "." && normalize_path(path)? != path {
        return Err("QA context paths must be canonical repository-relative scopes".into());
    }
    Ok(())
}
fn add(priorities: &mut BTreeMap<String, usize>, path: &str, priority: usize) -> Result<()> {
    canonical(path)?;
    priorities
        .entry(path.into())
        .and_modify(|rank| *rank = (*rank).min(priority))
        .or_insert(priority);
    Ok(())
}
fn overlaps(left: &str, right: &str) -> bool {
    scope_contains(left, right) || scope_contains(right, left)
}
fn path_sample(paths: &[String]) -> Vec<String> {
    let mut size = 0;
    paths
        .iter()
        .take(16)
        .take_while(|path| {
            size += path.len();
            size <= MAX_SCOPE_BYTES
        })
        .cloned()
        .collect()
}
fn prefix(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn query(parts: &[String]) -> Result<String> {
    // A valid plan can have more acceptance text than one 64 KiB request.
    // Project whole-word chunks first, then project their combined terms. Every
    // part participates; this is lexical sampling, never semantic coverage.
    let mut text = parts.join("\n");
    while text.len() > 65536 {
        let mut projected = Vec::new();
        let mut chunk = String::new();
        for word in text.split_whitespace() {
            if word.len() > 65536 {
                return Err("QA query contains an oversized atomic term".into());
            }
            if !chunk.is_empty() && chunk.len() + 1 + word.len() > 65536 {
                projected.push(retrieval::project(&chunk, None)?);
                chunk.clear();
            }
            if !chunk.is_empty() {
                chunk.push(' ');
            }
            chunk.push_str(word);
        }
        if !chunk.is_empty() {
            projected.push(retrieval::project(&chunk, None)?);
        }
        text = projected.join(" ");
    }
    retrieval::project(&text, None)
}

fn compact_profile(project: &Project, raw: Value, state: &Value, budget: usize) -> Result<Value> {
    let config = project.config()?;
    let approved: Vec<String> = if state.get("required_checks").is_some() {
        array(state, "required_checks")?
    } else {
        array(state, "checks")?
    };
    let names: BTreeSet<_> = approved.into_iter().collect();
    let configured = names
        .iter()
        .map(|name| {
            let check = config
                .checks
                .get(name)
                .ok_or("QA approved check is no longer configured")?;
            Ok(json!({"name":name,"argv":check.argv,"timeout_seconds":check.timeout_seconds}))
        })
        .collect::<Result<Vec<_>>>()?;
    let configs = raw["style"]["configs"].as_array().into_iter().flatten().take(8).map(|value| {
        let content = value["content"].as_str().unwrap_or("");
        let included = prefix(content, 512);
        json!({"path":value["path"],"scope":value["scope"],"selection":value["selection"],
            "content":included,"content_omitted_bytes":value["content_omitted_bytes"].as_u64().unwrap_or(0)+ (content.len()-included.len()) as u64})
    }).collect::<Vec<_>>();
    let coverage = &raw["coverage"];
    let mut out = json!({"snapshot":raw["snapshot"],
        "style":{"configs":configs,"precedence":"Explicit project rules govern; scoped configurations are static excerpts, native globs/inheritance unresolved."},
        "testing":{"configured":configured,"unconfigured":config.checks.is_empty(),
            "status":"Static discovery only; counts and configured argv are not execution evidence or coverage proof."},
        "coverage":{"indexed_files":coverage["indexed_files"],"snapshot_excluded":coverage["snapshot_excluded"],
            "source_files_in_scope":coverage["source_files_in_scope"],"style_configs_found":coverage["style_configs_found"],
            "tests_found":coverage["tests_found"],"candidates_found":coverage["candidates_found"],
            "configured_found":coverage["configured_found"],"approved_checks_found":names.len(),"complete":false}});
    loop {
        out["coverage"]["style_configs_omitted"] = json!(coverage["style_configs_found"]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(out["style"]["configs"].as_array().unwrap().len() as u64));
        out["coverage"]["approved_checks_omitted"] =
            json!(names.len() - out["testing"]["configured"].as_array().unwrap().len());
        if serde_json::to_vec(&out)?.len() <= budget {
            return Ok(out);
        }
        let configs = out["style"]["configs"].as_array_mut().unwrap();
        if let Some(value) = configs
            .iter_mut()
            .filter(|value| value["content"].as_str().is_some_and(|s| !s.is_empty()))
            .max_by_key(|value| value["content"].as_str().unwrap().len())
        {
            let content = value["content"].as_str().unwrap();
            let included = prefix(content, content.len() / 2).to_owned();
            value["content_omitted_bytes"] = json!(
                value["content_omitted_bytes"].as_u64().unwrap_or(0)
                    + (content.len() - included.len()) as u64
            );
            value["content"] = json!(included);
        } else if configs.pop().is_some() {
            continue;
        } else if out["testing"]["configured"]
            .as_array_mut()
            .unwrap()
            .pop()
            .is_some()
        {
            continue;
        } else {
            return Err("QA profile metadata exceeds its byte budget".into());
        }
    }
}

/// Called under the project lock. `paths` contains every requested scope for
/// complete scoped instruction discovery; only focus.selected_paths go to brief.
/// Neither an overflowing selection nor an empty legacy contract broadens to
/// the whole project implicitly. All source omission remains an explicit gap.
pub fn build(
    project: &Project,
    state: &Value,
    changed_paths: &[String],
    budget: usize,
) -> Result<Value> {
    let source_budget = (budget.saturating_mul(2) / 3).clamp(2048, 8000);
    let profile_budget = (budget / 4).clamp(2048, 4000);
    // Scope names are repeated in brief's header. Reserve most of a small
    // packet for source and explicit coverage instead of exhausting it on paths.
    let scope_byte_limit = (source_budget / 4).min(MAX_SCOPE_BYTES);
    let before = project.snapshot()?;
    if state["snapshot"].as_str() != Some(before.hash.as_str()) {
        return Err("QA context requires the current workflow/source snapshot".into());
    }
    let obligations: Vec<Obligation> = array(state, "verification")?;
    let tasks: Vec<Task> = array(state, "tasks")?;
    let acceptance: Vec<String> = array(state, "acceptance")?;
    let changed: BTreeSet<_> = changed_paths.iter().cloned().collect();
    let mut priorities = BTreeMap::<String, usize>::new();
    let mut responsible = BTreeSet::new();
    let mut query_parts = Vec::new();
    let mut criteria = BTreeSet::new();
    for obligation in &obligations {
        query_parts.push(obligation.risk.clone());
        for &index in &obligation.acceptance {
            let text = acceptance
                .get(index)
                .ok_or("QA obligation references an absent acceptance criterion")?;
            if criteria.insert(index) {
                query_parts.push(text.clone());
            }
        }
        responsible.extend(obligation.tasks.iter().cloned());
        for path in &obligation.paths {
            add(&mut priorities, path, 0)?;
        }
    }
    for path in &changed {
        add(&mut priorities, path, 1)?;
    }
    for task in &tasks {
        if responsible.contains(&task.id)
            || task
                .paths
                .iter()
                .any(|scope| changed.iter().any(|path| overlaps(scope, path)))
        {
            for path in &task.context {
                add(&mut priorities, path, 2)?;
            }
        }
    }
    if priorities.is_empty() {
        for task in &tasks {
            for path in task.paths.iter().chain(&task.context) {
                add(&mut priorities, path, 2)?;
            }
        }
    }
    if priorities.is_empty() {
        return Err(
            "QA has no changed or declared verification/task scopes; no whole-project fallback"
                .into(),
        );
    }
    if priorities.len() > MAX_REQUESTED_PATHS
        || priorities.keys().map(String::len).sum::<usize>() > MAX_REQUESTED_BYTES
    {
        return Err(
            "QA instruction scopes exceed 1024 paths or 65536 bytes; narrow the task contract"
                .into(),
        );
    }
    let paths = priorities.keys().cloned().collect::<Vec<_>>();
    // Collapse only to a prefix explicitly requested in the contract/context.
    // That prefix inherits the best priority of the exact paths it covers.
    let mut reduced = Vec::new();
    for (path, priority) in &priorities {
        if priorities
            .keys()
            .any(|parent| parent != path && scope_contains(parent, path))
        {
            continue;
        }
        let rank = priorities
            .iter()
            .filter(|(child, _)| scope_contains(path, child))
            .map(|(_, rank)| *rank)
            .min()
            .unwrap_or(*priority);
        reduced.push((rank, path.clone()));
    }
    reduced.sort();
    let mut selected = Vec::new();
    let mut scope_bytes = 0;
    for (_, path) in reduced {
        if selected.len() < MAX_SCOPES && scope_bytes + path.len() <= scope_byte_limit {
            scope_bytes += path.len();
            selected.push(path);
        }
    }
    if selected.is_empty() {
        return Err("No QA scope fits the reserved source-header budget; increase context budget or narrow scopes".into());
    }
    let omitted = paths
        .iter()
        .filter(|path| !selected.iter().any(|scope| scope_contains(scope, path)))
        .cloned()
        .collect::<Vec<_>>();
    if obligations.is_empty() {
        query_parts.push(state["goal"].as_str().unwrap_or("").to_owned());
    }
    // Paths add literal anchors; they do not imply semantic understanding.
    query_parts.extend(selected.iter().cloned());
    let query = query(&query_parts)?;
    let mut options = Vec::new();
    for path in &selected {
        options.extend(["--path".into(), path.clone()]);
    }
    let mut profile_options = options.clone();
    profile_options.extend(["--budget".into(), "24000".into()]);
    let raw: Value = serde_json::from_str(&profile::run(project, &profile_options)?)?;
    let profile = compact_profile(project, raw, state, profile_budget)?;
    options.extend(["--budget".into(), source_budget.to_string()]);
    if retrieval::has_terms(&query) {
        options.extend(["--impact".into(), "--".into(), query.clone()]);
    }
    let source = brief::run(project, &options)?;
    let snapshot = project.snapshot()?;
    if profile["snapshot"].as_str() != Some(before.hash.as_str())
        || snapshot.hash != before.hash
        || !source.starts_with(&format!("snapshot {}\n", before.hash))
    {
        return Err(
            "Source/config changed during QA context preparation; retry against current state"
                .into(),
        );
    }
    let missing = selected
        .iter()
        .filter(|scope| {
            !snapshot
                .files
                .keys()
                .any(|path| scope_contains(scope, path))
        })
        .cloned()
        .collect::<Vec<_>>();
    let ids = obligations
        .iter()
        .filter(|item| {
            item.paths
                .iter()
                .any(|path| selected.iter().any(|scope| overlaps(scope, path)))
        })
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let focus = json!({"selected_obligation_ids":ids,"selected_paths":selected,"total_paths":paths.len(),
        "omitted_paths":path_sample(&omitted),"omitted_path_count":omitted.len(),
        "covered_path_count":paths.len()-omitted.len(),
        "missing_paths":path_sample(&missing),"missing_path_count":missing.len(),
        "query":query,"source_budget":source_budget,"profile_budget":profile_budget,
        "scope_byte_limit":scope_byte_limit,
        "changed_path_count":changed.len(),"current_changed_count":changed.iter().filter(|path| snapshot.files.contains_key(*path)).count(),
        "status":"Targeted static navigation; omitted paths/brief excerpts remain read gaps, not coverage proof."});
    Ok(json!({"profile":profile,"source":source,"paths":paths,"focus":focus}))
}
