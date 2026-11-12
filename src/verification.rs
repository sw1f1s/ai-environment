//! Change-specific verification contracts and source-bound QA evidence.
//! A passing configured command is execution evidence, not proof that its tests
//! exercised a particular invariant. QA owns that semantic assessment.
use crate::{
    project::{normalize_path, scope_contains, sha, valid_id, Project, Snapshot},
    verify, Result,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fs};

pub const MAX_OBLIGATIONS: usize = 6;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Obligation {
    pub id: String,
    pub acceptance: Vec<usize>,
    pub tasks: Vec<String>,
    pub paths: Vec<String>,
    pub risk: String,
    pub checks: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub path: String,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationResult {
    pub id: String,
    pub status: String,
    pub proofs: Vec<String>,
    pub locations: Vec<Location>,
    pub evidence: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredReport {
    pub snapshot: String,
    pub contract_hash: String,
    pub results: Vec<VerificationResult>,
}

pub(crate) struct TaskScope<'a> {
    pub id: &'a str,
    pub paths: &'a [String],
    pub context: &'a [String],
}

pub(crate) struct Feedback {
    pub results: Vec<VerificationResult>,
    pub findings: Vec<String>,
    pub tasks: Vec<String>,
}

fn text_valid(text: &str) -> bool {
    !text.trim().is_empty() && !text.contains('\0') && text.chars().count() <= 768
}
fn unique<T: Ord>(items: &[T]) -> bool {
    items.iter().collect::<BTreeSet<_>>().len() == items.len()
}
fn prefix(project: &Project, path: &str) -> Result<()> {
    if path.len() > 512
        || path.contains(['*', '?', '[', ']'])
        || (path != "." && normalize_path(path)? != path)
    {
        return Err(
            "Verification paths must be canonical file/directory prefixes up to 512 bytes".into(),
        );
    }
    if path != "." {
        if project.root.join(path).is_dir() {
            project.resolve(&format!("{path}/.aienv-verification-scope-validation"))?;
        } else {
            project.resolve(path)?;
        }
    }
    Ok(())
}
fn task_contains(tasks: &[TaskScope<'_>], ids: &[String], path: &str) -> bool {
    tasks.iter().any(|task| {
        ids.iter().any(|id| id == task.id)
            && task
                .paths
                .iter()
                .chain(task.context)
                .any(|scope| scope_contains(scope, path))
    })
}

pub(crate) fn validate_plan(
    project: &Project,
    obligations: &[Obligation],
    acceptance_count: usize,
    tasks: &[TaskScope<'_>],
    approved_checks: &[String],
) -> Result<()> {
    // Existing manual hosts may omit this additive contract entirely.
    if obligations.is_empty() {
        return Ok(());
    }
    if obligations.len() > MAX_OBLIGATIONS {
        return Err("Verification plan supports at most 6 obligations".into());
    }
    let mut ids = BTreeSet::new();
    let mut covered = BTreeSet::new();
    for obligation in obligations {
        if !valid_id(&obligation.id)
            || !ids.insert(&obligation.id)
            || obligation.acceptance.is_empty()
            || obligation.acceptance.len() > 64
            || !unique(&obligation.acceptance)
            || obligation.acceptance.iter().any(|&i| i >= acceptance_count)
            || obligation.tasks.is_empty()
            || obligation.tasks.len() > 64
            || !unique(&obligation.tasks)
            || obligation
                .tasks
                .iter()
                .any(|id| !tasks.iter().any(|task| task.id == id))
            || obligation.paths.is_empty()
            || obligation.paths.len() > 16
            || !unique(&obligation.paths)
            || !text_valid(&obligation.risk)
            || obligation.checks.len() > 64
            || !unique(&obligation.checks)
            || obligation
                .checks
                .iter()
                .any(|name| !approved_checks.contains(name))
            || (!approved_checks.is_empty() && obligation.checks.is_empty())
        {
            return Err("Verification obligation needs a unique ID, valid acceptance indices/task IDs, scoped paths, bounded risk and approved check names".into());
        }
        for path in &obligation.paths {
            prefix(project, path)?;
            if !task_contains(tasks, &obligation.tasks, path) {
                return Err(
                    "Verification obligation path is outside its tasks' write/read scopes".into(),
                );
            }
        }
        covered.extend(obligation.acceptance.iter().copied());
    }
    if covered.len() != acceptance_count {
        return Err("Verification obligations must cover every acceptance criterion".into());
    }
    Ok(())
}

fn current_proof(
    project: &Project,
    snapshot: &str,
    approved_checks: &[String],
    config: &crate::project::Config,
    config_hash: &str,
    id: &str,
    require_pass: bool,
) -> Result<String> {
    let proof = verify::load(project, id)?;
    let check = config
        .checks
        .get(&proof.check)
        .ok_or("Verification proof refers to an unconfigured check")?;
    if !approved_checks.contains(&proof.check)
        || proof.snapshot != snapshot
        || proof.snapshot_after.as_deref() != Some(snapshot)
        || proof.config_hash != config_hash
        || proof.argv != check.argv
        || !proof.source_unchanged
        || (require_pass
            && (!proof.passed
                || proof.exit_code != Some(0)
                || proof.timed_out
                || proof.output_limited
                || !proof.capture_complete
                || proof.execution_error.is_some()))
    {
        return Err("Verification evidence requires actual fresh source/configuration proofs; verified requires passing complete execution".into());
    }
    Ok(proof.check)
}

pub(crate) fn assess(
    project: &Project,
    snapshot: &Snapshot,
    obligations: &[Obligation],
    tasks: &[TaskScope<'_>],
    approved_checks: &[String],
    results: &[VerificationResult],
    qa_proofs: &[String],
    verdict: &str,
) -> Result<Feedback> {
    if results.len() != obligations.len() || results.len() > MAX_OBLIGATIONS {
        return Err("QA verification must report every obligation exactly once".into());
    }
    let config = project.config()?;
    let config_hash = sha(fs::read(project.state.join("config.json"))?);
    let mut ids = BTreeSet::new();
    let mut findings = Vec::new();
    let mut affected = BTreeSet::new();
    for result in results {
        let obligation = obligations
            .iter()
            .find(|o| o.id == result.id)
            .ok_or("QA verification refers to an unknown obligation")?;
        if !ids.insert(&result.id)
            || !matches!(result.status.as_str(), "verified" | "gap" | "inspected")
            || !text_valid(&result.evidence)
            || result.proofs.len() > 64
            || !unique(&result.proofs)
            || result.proofs.iter().any(|id| !qa_proofs.contains(id))
            || result.locations.len() > 8
            || result
                .locations
                .iter()
                .map(|l| (&l.path, l.line))
                .collect::<BTreeSet<_>>()
                .len()
                != result.locations.len()
            || (result.status != "gap" && result.locations.is_empty())
        {
            return Err("QA verification needs unique obligation IDs, verified/gap/inspected status, bounded evidence, actual QA proof IDs and grounded locations".into());
        }
        for location in &result.locations {
            if location.path.len() > 512 || normalize_path(&location.path)? != location.path {
                return Err("Verification evidence needs canonical indexed file locations".into());
            }
            project.resolve(&location.path)?;
            let file = snapshot
                .files
                .get(&location.path)
                .ok_or("Verification location is not an indexed file")?;
            if location.line == 0
                || location.line > file.text.lines().count()
                || !obligation
                    .paths
                    .iter()
                    .any(|path| scope_contains(path, &location.path))
                || !task_contains(tasks, &obligation.tasks, &location.path)
            {
                return Err(
                    "Verification evidence line must exist within its obligation and task scopes"
                        .into(),
                );
            }
        }
        match result.status.as_str() {
            "inspected" if !approved_checks.is_empty() || !result.proofs.is_empty() => return Err(
                "Inspected means static-only evidence, with no approved checks or execution proofs"
                    .into(),
            ),
            "verified" if approved_checks.is_empty() => {
                return Err(
                    "No approved checks: report inspected (static-only) or a gap, never verified"
                        .into(),
                )
            }
            "gap" => {
                if verdict != "changes" {
                    return Err("Verification gaps require verdict changes".into());
                }
                findings.push(format!("Verification {}: {}", result.id, result.evidence));
                affected.extend(obligation.tasks.iter().cloned());
            }
            _ => (),
        }
        let mut checked = BTreeSet::new();
        for id in &result.proofs {
            checked.insert(current_proof(
                project,
                &snapshot.hash,
                approved_checks,
                &config,
                &config_hash,
                id,
                result.status == "verified",
            )?);
        }
        if result.status == "verified"
            && obligation.checks.iter().any(|name| !checked.contains(name))
        {
            return Err(
                "Verified obligation lacks a fresh passing proof for its named check".into(),
            );
        }
    }
    Ok(Feedback {
        results: obligations
            .iter()
            .map(|o| results.iter().find(|r| r.id == o.id).unwrap().clone())
            .collect(),
        findings,
        tasks: affected.into_iter().collect(),
    })
}
