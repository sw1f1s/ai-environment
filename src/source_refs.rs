//! Native QA references to source fragments actually rendered by trusted brief.
//! Checksums detect accidental corruption/mixed artifacts, not hostile host-state
//! writers. Grounded locations do not establish semantic coverage or test execution.
use crate::{
    project::{normalize_path, scope_contains, sha, valid_id, Snapshot},
    verification::Obligation,
    Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const SCHEMA: u32 = 1;
const MAX_REFS: usize = 128;
const MAX_PUBLIC: usize = 16 * 1024;
const MAX_PRIVATE: usize = 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Task {
    id: String,
    title: String,
    instructions: String,
    paths: Vec<String>,
    #[serde(default)]
    context: Vec<String>,
    depends: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Contract {
    goal: String,
    architecture: Value,
    obligations: Vec<Obligation>,
    tasks: Vec<Task>,
    acceptance: Vec<String>,
    checks: Vec<String>,
    required_checks: Vec<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Card {
    id: String,
    path: String,
    line: usize,
    end_line: usize,
    obligations: Vec<String>,
    file_hash: String,
    text_hash: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    snapshot: String,
    source_hash: String,
    contract: Contract,
    contract_hash: String,
    refs: Vec<Card>,
    omitted_refs: usize,
    excluded_fragments: usize,
    digest: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerRow {
    id: String,
    status: String,
    proofs: Vec<String>,
    source_refs: Vec<String>,
    evidence: String,
}

fn hash_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn prefix(path: &str) -> Result<()> {
    if path != "." && normalize_path(path)? != path {
        return Err("Source reference scopes must be canonical project paths".into());
    }
    Ok(())
}
fn eligible(contract: &Contract, path: &str) -> Vec<String> {
    contract
        .obligations
        .iter()
        .filter(|obligation| {
            obligation
                .paths
                .iter()
                .any(|scope| scope_contains(scope, path))
                && contract.tasks.iter().any(|task| {
                    obligation.tasks.contains(&task.id)
                        && task
                            .paths
                            .iter()
                            .chain(&task.context)
                            .any(|scope| scope_contains(scope, path))
                })
        })
        .map(|item| item.id.clone())
        .collect()
}
fn contract_valid(contract: &Contract) -> Result<()> {
    if contract.obligations.len() > 6
        || contract.tasks.len() > 64
        || contract.acceptance.len() > 64
        || contract.checks.len() > 64
        || contract.required_checks.len() > 64
        || contract
            .required_checks
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            != contract.required_checks.len()
    {
        return Err("Source reference contract exceeds task/obligation bounds".into());
    }
    let mut tasks = BTreeSet::new();
    for task in &contract.tasks {
        if !valid_id(&task.id)
            || !tasks.insert(&task.id)
            || task.paths.is_empty()
            || task.paths.len() > 64
            || task.context.len() > 64
            || task.depends.len() > 64
        {
            return Err("Invalid task scope in source reference contract".into());
        }
        for path in task.paths.iter().chain(&task.context) {
            prefix(path)?;
        }
    }
    let mut ids = BTreeSet::new();
    for obligation in &contract.obligations {
        if !valid_id(&obligation.id)
            || !ids.insert(&obligation.id)
            || obligation.paths.is_empty()
            || obligation.paths.len() > 16
            || obligation.tasks.is_empty()
            || obligation.tasks.len() > 64
            || obligation.tasks.iter().any(|id| !tasks.contains(id))
            || obligation.tasks.iter().collect::<BTreeSet<_>>().len() != obligation.tasks.len()
            || obligation.acceptance.is_empty()
            || obligation
                .acceptance
                .iter()
                .any(|&n| n >= contract.acceptance.len())
        {
            return Err("Invalid obligation in source reference contract".into());
        }
        for path in &obligation.paths {
            prefix(path)?;
        }
    }
    Ok(())
}
fn state_contract(state: &Value) -> Result<Contract> {
    fn array<T: serde::de::DeserializeOwned>(state: &Value, name: &str) -> Result<Vec<T>> {
        match state.get(name) {
            None | Some(Value::Null) => Ok(Vec::new()),
            Some(value) => Ok(serde_json::from_value(value.clone())?),
        }
    }
    let contract = Contract {
        goal: match state.get("goal") {
            None | Some(Value::Null) => String::new(),
            Some(value) => serde_json::from_value(value.clone())?,
        },
        architecture: state.get("architecture").cloned().unwrap_or(Value::Null),
        obligations: array(state, "verification")?,
        tasks: array(state, "tasks")?,
        acceptance: array(state, "acceptance")?,
        checks: array(state, "checks")?,
        required_checks: array(state, "required_checks")?,
    };
    contract_valid(&contract)?;
    Ok(contract)
}
fn identity(snapshot: &str, contract_hash: &str, card: &Card) -> Result<String> {
    Ok(format!(
        "s{}",
        &sha(serde_json::to_vec(&(
            "qa-source-ref-1",
            snapshot,
            contract_hash,
            &card.path,
            card.line,
            card.end_line,
            &card.file_hash,
            &card.text_hash
        ))?)[..10]
    ))
}
fn checksum(manifest: &Manifest) -> Result<String> {
    Ok(sha(serde_json::to_vec(&(
        "qa-source-catalog-1",
        manifest.schema,
        &manifest.snapshot,
        &manifest.source_hash,
        &manifest.contract,
        &manifest.contract_hash,
        &manifest.refs,
        manifest.omitted_refs,
        manifest.excluded_fragments,
    ))?))
}
fn validate(value: &Value) -> Result<Manifest> {
    if serde_json::to_vec(value)?.len() > MAX_PRIVATE {
        return Err("Source reference manifest exceeds 1 MiB".into());
    }
    let manifest: Manifest = serde_json::from_value(value.clone())
        .map_err(|e| format!("Missing/invalid private source reference catalog: {e}"))?;
    if manifest.schema != SCHEMA
        || !hash_valid(&manifest.snapshot)
        || !hash_valid(&manifest.source_hash)
        || !hash_valid(&manifest.contract_hash)
        || !hash_valid(&manifest.digest)
        || manifest.refs.len() > MAX_REFS
    {
        return Err("Invalid source reference catalog header".into());
    }
    contract_valid(&manifest.contract)?;
    if sha(serde_json::to_vec(&manifest.contract)?) != manifest.contract_hash
        || checksum(&manifest)? != manifest.digest
    {
        return Err("Corrupt or mixed source reference catalog checksum/contract".into());
    }
    let mut ids = BTreeSet::new();
    let mut ranges = BTreeSet::new();
    for card in &manifest.refs {
        prefix(&card.path)?;
        if card.path == "."
            || card.path.len() > 512
            || card.line == 0
            || card.end_line < card.line
            || !hash_valid(&card.file_hash)
            || !hash_valid(&card.text_hash)
            || card.obligations.is_empty()
            || card.obligations != eligible(&manifest.contract, &card.path)
            || identity(&manifest.snapshot, &manifest.contract_hash, card)? != card.id
            || !ids.insert(&card.id)
            || !ranges.insert((&card.path, card.line, card.end_line))
        {
            return Err("Invalid source reference card identity/range/scope".into());
        }
    }
    if serde_json::to_vec(&render(&manifest))?.len() > MAX_PUBLIC {
        return Err("Public source reference metadata exceeds 16 KiB".into());
    }
    Ok(manifest)
}
fn render(manifest: &Manifest) -> Value {
    let by_obligation = manifest
        .contract
        .obligations
        .iter()
        .map(|item| {
            (
                item.id.clone(),
                json!(manifest
                    .refs
                    .iter()
                    .filter(|card| card.obligations.contains(&item.id))
                    .map(|card| &card.id)
                    .collect::<Vec<_>>()),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let gaps = by_obligation
        .iter()
        .filter(|(_, ids)| ids.as_array().is_some_and(Vec::is_empty))
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    json!({"refs":manifest.refs.iter().map(|card| json!({"id":card.id,"path":card.path,
        "line":card.line,"end_line":card.end_line,"obligations":card.obligations})).collect::<Vec<_>>(),
        "by_obligation":by_obligation,"omitted_refs":manifest.omitted_refs,"excluded_fragments":manifest.excluded_fragments,
        "obligations_without_refs":gaps,"status":"Visible source fragments only; missing/omitted references are evidence gaps. A reference does not prove semantic coverage or test execution."})
}

fn read_line(source: &str, position: usize) -> (&str, usize) {
    match source[position..].find('\n') {
        Some(length) => (&source[position..position + length], position + length + 1),
        None => (&source[position..], source.len()),
    }
}
enum Header<'a> {
    Full(&'a str, usize, Option<&'a str>),
    Excerpt(&'a str, Option<&'a str>),
}
fn header(line: &str) -> Result<Option<Header<'_>>> {
    let Some(body) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return Ok(None);
    };
    let (body, excerpt) = match body.strip_suffix("; omitted lines require source inspection") {
        Some(body) => (body, true),
        None => (body, false),
    };
    let (body, hash) = match body.rsplit_once(" hash=") {
        Some((body, hash)) => {
            if !hash_valid(hash) {
                return Err("Malformed source block hash".into());
            }
            (body, Some(hash))
        }
        None => (body, None),
    };
    if excerpt {
        let path = body
            .strip_suffix(" excerpt")
            .ok_or("Malformed excerpt header")?;
        return Ok(Some(Header::Excerpt(path, hash)));
    }
    if let Some((path, number)) = body.rsplit_once(":1..") {
        let end = number
            .parse::<usize>()
            .map_err(|_| "Malformed full source range")?;
        if end.to_string() != number {
            return Err("Noncanonical full source range".into());
        }
        return Ok(Some(Header::Full(path, end, hash)));
    }
    Ok(None)
}

/// Capture only exact full-file bodies or numbered excerpt runs actually present
/// in trusted brief. Never guess an unseen location when a card is unavailable.
pub fn capture(snapshot: &Snapshot, state: &Value, source: &str) -> Result<Value> {
    if !hash_valid(&snapshot.hash)
        || state["snapshot"].as_str() != Some(snapshot.hash.as_str())
        || !source.starts_with(&format!("snapshot {}\n", snapshot.hash))
        || source.len() > 32000
    {
        return Err("Source reference capture requires the exact current brief snapshot".into());
    }
    let contract = state_contract(state)?;
    let contract_hash = sha(serde_json::to_vec(&contract)?);
    let mut cards = BTreeMap::<(String, usize, usize), Card>::new();
    let mut seen_files = BTreeSet::new();
    let mut excluded = 0;
    let mut position = 0;
    while position < source.len() {
        let (line, next) = read_line(source, position);
        position = next;
        let Some(parsed) = header(line)? else {
            continue;
        };
        let (path, hash) = match parsed {
            Header::Full(path, _, hash) | Header::Excerpt(path, hash) => (path, hash),
        };
        prefix(path)?;
        let file = snapshot
            .files
            .get(path)
            .ok_or("Source block is not an indexed snapshot file")?;
        if file.path != path
            || sha(file.text.as_bytes()) != file.hash
            || hash.is_some_and(|hash| hash != file.hash)
            || !seen_files.insert(path.to_owned())
        {
            return Err("Corrupt/repeated source block or mismatched original file hash".into());
        }
        let lines = file.text.lines().collect::<Vec<_>>();
        let mut runs = Vec::new();
        match parsed {
            Header::Full(_, end, _) => {
                if end != lines.len() {
                    return Err("Full source header does not match original line count".into());
                }
                let rendered = format!(
                    "{}{}",
                    file.text,
                    if file.text.ends_with('\n') { "" } else { "\n" }
                );
                if !source[position..].starts_with(&rendered) {
                    return Err("Full visible source body does not match original snapshot".into());
                }
                position += rendered.len();
                if end > 0 {
                    runs.push((1, end));
                } else {
                    excluded += 1;
                }
            }
            Header::Excerpt(_, _) => {
                let mut start = 0;
                let mut last = 0;
                while position < source.len() {
                    let (line, next) = read_line(source, position);
                    if line.is_empty() {
                        break;
                    }
                    let (number, text) = line
                        .split_once(':')
                        .ok_or("Malformed numbered source excerpt")?;
                    let number = number
                        .parse::<usize>()
                        .map_err(|_| "Malformed excerpt line number")?;
                    if number == 0
                        || number <= last
                        || number > lines.len()
                        || line.split_once(':').unwrap().0 != number.to_string()
                        || lines[number - 1] != text
                    {
                        return Err(
                            "Visible excerpt line does not exactly match original snapshot".into(),
                        );
                    }
                    if start == 0 {
                        start = number;
                    } else if number != last + 1 {
                        runs.push((start, last));
                        start = number;
                    }
                    last = number;
                    position = next;
                }
                if start == 0 {
                    return Err("Empty source excerpt has no visible reference".into());
                }
                runs.push((start, last));
            }
        }
        let owners = eligible(&contract, path);
        for (line, end_line) in runs {
            if owners.is_empty() || path.len() > 512 {
                excluded += 1;
                continue;
            }
            let mut card = Card {
                id: String::new(),
                path: path.into(),
                line,
                end_line,
                obligations: owners.clone(),
                file_hash: file.hash.clone(),
                text_hash: sha(lines[line - 1..end_line].join("\n")),
            };
            card.id = identity(&snapshot.hash, &contract_hash, &card)?;
            cards.insert((path.into(), line, end_line), card);
        }
    }
    // Give each obligation an available card before filling additional runs.
    let mut available = cards.into_values().collect::<Vec<_>>();
    let total = available.len();
    let mut refs = Vec::new();
    loop {
        let mut progress = false;
        for obligation in &contract.obligations {
            if refs.len() >= MAX_REFS {
                break;
            }
            if let Some(index) = available
                .iter()
                .position(|card| card.obligations.contains(&obligation.id))
            {
                refs.push(available.remove(index));
                progress = true;
            }
        }
        if !progress || refs.len() >= MAX_REFS {
            break;
        }
    }
    let mut manifest = Manifest {
        schema: SCHEMA,
        snapshot: snapshot.hash.clone(),
        source_hash: sha(source),
        contract,
        contract_hash,
        refs,
        omitted_refs: 0,
        excluded_fragments: excluded,
        digest: String::new(),
    };
    loop {
        manifest.omitted_refs = total - manifest.refs.len();
        if serde_json::to_vec(&render(&manifest))?.len() <= MAX_PUBLIC {
            break;
        }
        if manifest.refs.pop().is_none() {
            return Err("Source reference contract metadata exceeds 16 KiB".into());
        }
    }
    manifest.digest = checksum(&manifest)?;
    let value = serde_json::to_value(&manifest)?;
    validate(&value)?;
    Ok(value)
}

/// Bounded model-visible metadata only; private digests/contracts never leak.
pub fn public(manifest: &Value) -> Result<Value> {
    Ok(render(&validate(manifest)?))
}

/// Check a saved dispatch contract against current workflow metadata without
/// recapturing files or trusting source freshness to imply contract freshness.
pub fn validate_contract(manifest: &Value, state: &Value) -> Result<()> {
    let manifest = validate(manifest)?;
    let contract = state_contract(state)?;
    if state["snapshot"].as_str() != Some(manifest.snapshot.as_str())
        || sha(serde_json::to_vec(&contract)?) != manifest.contract_hash
    {
        return Err(
            "QA source reference contract changed since dispatch; do not replay stale evidence"
                .into(),
        );
    }
    Ok(())
}

/// Convert native reference IDs to legacy grounded locations using only the
/// persisted dispatch catalog. Existing manual workflow locations are untouched.
pub fn bind(answer: &Value, manifest: &Value, entry_snapshot: &str) -> Result<Value> {
    let manifest = validate(manifest)?;
    if manifest.snapshot != entry_snapshot || answer["snapshot"].as_str() != Some(entry_snapshot) {
        return Err(
            "QA source reference catalog/report belongs to a different dispatch snapshot".into(),
        );
    }
    let rows = answer["verification"]
        .as_array()
        .ok_or("Native QA requires verification source_refs")?;
    if rows.len() != manifest.contract.obligations.len() {
        return Err("Native QA must report each source-reference obligation once".into());
    }
    let cards = manifest
        .refs
        .iter()
        .map(|card| (card.id.as_str(), card))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut bound = Vec::new();
    for row in rows {
        if row.get("locations").is_some() {
            return Err(
                "Native QA must return source_refs IDs, never model-authored locations".into(),
            );
        }
        let parsed: AnswerRow = serde_json::from_value(row.clone())?;
        if !manifest
            .contract
            .obligations
            .iter()
            .any(|o| o.id == parsed.id)
            || !seen.insert(parsed.id.clone())
            || !matches!(parsed.status.as_str(), "verified" | "inspected" | "gap")
            || parsed.source_refs.len() > 8
            || (parsed.status != "gap" && parsed.source_refs.is_empty())
        {
            return Err("QA verification requires unique declared obligations, a valid status and actual visible source refs".into());
        }
        let mut refs = BTreeSet::new();
        let mut locations = Vec::new();
        for id in &parsed.source_refs {
            let card = cards
                .get(id.as_str())
                .ok_or("Unknown or omitted QA source reference ID")?;
            if !refs.insert(id) || !card.obligations.contains(&parsed.id) {
                return Err("Duplicate or wrong-obligation QA source reference ID".into());
            }
            locations.push(json!({"path":card.path,"line":card.line}));
        }
        // Proof freshness and the meaning/completeness of evidence remain the
        // authoritative verification controller's responsibility.
        bound.push(
            json!({"id":parsed.id,"status":parsed.status,"proofs":parsed.proofs,
            "locations":locations,"evidence":parsed.evidence}),
        );
    }
    let mut answer = answer.clone();
    answer
        .as_object_mut()
        .ok_or("QA answer must be an object")?
        .insert("verification".into(), json!(bound));
    Ok(answer)
}
