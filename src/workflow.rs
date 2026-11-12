//! Durable task/attempt scheduling and source-bound integration gates.
use crate::{
    edit, json,
    project::{normalize_path, scope_contains, sha, valid_id, Project, Snapshot},
    verification, verify, Result,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json as value, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
};

const MAX_REPORT: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    id: String,
    title: String,
    instructions: String,
    paths: Vec<String>,
    #[serde(default)]
    context: Vec<String>,
    depends: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    snapshot: String,
    tasks: Vec<Job>,
    checks: Vec<String>,
    acceptance: Vec<String>,
    #[serde(default)]
    architecture: Option<ArchitectureContract>,
    #[serde(default)]
    verification: Vec<verification::Obligation>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchitectureContract {
    boundaries: Vec<String>,
    dependencies: Vec<String>,
    extension_scenarios: Vec<ExtensionScenario>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtensionScenario {
    change: String,
    expected_paths: Vec<String>,
    preserved_paths: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchitectureReport {
    assessment: Vec<String>,
    issues: Vec<ArchitectureIssue>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchitectureIssue {
    severity: String,
    tasks: Vec<String>,
    path: String,
    line: usize,
    problem: String,
    consequence: String,
    fix: String,
    verification: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredArchitectureReport {
    snapshot: String,
    contract_hash: String,
    assessment: Vec<String>,
    issues: Vec<ArchitectureIssue>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    id: String,
    goal: String,
    phase: String,
    mode: String,
    snapshot: String,
    workers: usize,
    round: usize,
    rounds: usize,
    required_checks: Vec<String>,
    plan: Option<Plan>,
    completed: BTreeMap<String, String>,
    summaries: BTreeMap<String, String>,
    findings: Vec<String>,
    review_snapshot: Option<String>,
    proofs: Vec<String>,
    task_states: BTreeMap<String, TaskProgress>,
    attempts: Vec<Attempt>,
    pending: Option<Pending>,
    failures: BTreeSet<String>,
    assessment: Vec<String>,
    failed_proofs: Vec<String>,
    #[serde(default)]
    last_architecture: Option<StoredArchitectureReport>,
    #[serde(default)]
    last_verification: Option<verification::StoredReport>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskProgress {
    status: String,
    generation: usize,
    input: Option<String>,
    findings: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    id: String,
    task: String,
    owner: String,
    snapshot: String,
    inputs: String,
    generation: usize,
    status: String,
    error: Option<String>,
    summary: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    before: String,
    after: String,
    jobs: Vec<Changes>,
    attempts: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Batch {
    snapshot: String,
    jobs: Vec<Changes>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Changes {
    id: String,
    summary: String,
    files: Vec<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    snapshot: String,
    verdict: String,
    findings: Vec<String>,
    #[serde(default)]
    tasks: Vec<String>,
    #[serde(default)]
    assessment: Vec<String>,
    #[serde(default)]
    architecture: Option<ArchitectureReport>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Qa {
    snapshot: String,
    verdict: String,
    findings: Vec<String>,
    proofs: Vec<String>,
    #[serde(default)]
    tasks: Vec<String>,
    #[serde(default)]
    assessment: Vec<String>,
    #[serde(default)]
    architecture: Option<ArchitectureReport>,
    #[serde(default)]
    verification: Option<Vec<verification::VerificationResult>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeEdit {
    path: String,
    expected_hash: Value,
    source: Option<String>,
    old: Option<String>,
    new: Option<String>,
    replacements: Option<Vec<Replacement>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Replacement {
    old: String,
    new: String,
}
fn filename(id: &str) -> Result<String> {
    if !valid_id(id) {
        return Err("Invalid workflow ID".into());
    }
    Ok(format!("workflows/{id}.json"))
}
fn text_valid(text: &str, limit: usize) -> bool {
    !text.trim().is_empty() && text.len() <= limit && !text.contains('\0')
}
fn hash_valid(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}
fn contract_text_valid(text: &str) -> bool {
    !text.trim().is_empty() && !text.contains('\0') && text.chars().count() <= 1024
}
fn architecture_text_valid(text: &str) -> bool {
    !text.trim().is_empty() && !text.contains('\0') && text.chars().count() <= 768
}
fn architecture_prefix(project: &Project, path: &str) -> Result<()> {
    if path.len() > 512
        || path.contains(['*', '?', '[', ']'])
        || (path != "." && normalize_path(path)? != path)
    {
        return Err(
            "Architecture paths must be canonical file/directory prefixes up to 512 bytes".into(),
        );
    }
    if path != "." {
        if project.root.join(path).is_dir() {
            project.resolve(&format!("{path}/.aienv-architecture-scope-validation"))?;
        } else {
            project.resolve(path)?;
        }
    }
    Ok(())
}
fn validate_architecture_contract(
    project: &Project,
    contract: &ArchitectureContract,
) -> Result<()> {
    if contract.boundaries.is_empty()
        || contract.boundaries.len() > 8
        || contract.dependencies.len() > 8
        || contract
            .boundaries
            .iter()
            .chain(&contract.dependencies)
            .any(|entry| !contract_text_valid(entry))
        || contract.extension_scenarios.len() > 3
    {
        return Err("Architecture contract needs 1..8 boundaries, 0..8 dependencies, bounded text and 0..3 extension scenarios".into());
    }
    for scenario in &contract.extension_scenarios {
        if !contract_text_valid(&scenario.change)
            || scenario.expected_paths.is_empty()
            || scenario.expected_paths.len() > 16
            || scenario.preserved_paths.len() > 16
        {
            return Err("Extension scenario needs a bounded change, 1..16 expected paths and 0..16 preserved paths".into());
        }
        for paths in [&scenario.expected_paths, &scenario.preserved_paths] {
            if paths.iter().collect::<BTreeSet<_>>().len() != paths.len() {
                return Err("Extension scenario paths must be unique".into());
            }
            for path in paths {
                architecture_prefix(project, path)?;
            }
        }
    }
    Ok(())
}
fn validate_plan(project: &Project, plan: &Plan, required_checks: &[String]) -> Result<()> {
    if !hash_valid(&plan.snapshot)
        || plan.tasks.is_empty()
        || plan.tasks.len() > 64
        || plan.acceptance.is_empty()
        || plan.acceptance.len() > 64
        || plan.acceptance.iter().any(|s| !text_valid(s, 4096))
    {
        return Err(
            "Plan needs 1..64 tasks and acceptance criteria, with a source snapshot".into(),
        );
    }
    if let Some(contract) = &plan.architecture {
        validate_architecture_contract(project, contract)?;
    }
    let configured = project
        .config()?
        .checks
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let required = required_checks.iter().cloned().collect::<BTreeSet<_>>();
    let checks = plan.checks.iter().cloned().collect::<BTreeSet<_>>();
    if checks.len() != plan.checks.len() || checks != required || !checks.is_subset(&configured) {
        return Err("Plan checks must list every approved workflow check exactly once".into());
    }
    let mut ids = BTreeSet::new();
    for job in &plan.tasks {
        if !valid_id(&job.id)
            || !ids.insert(job.id.clone())
            || !text_valid(&job.title, 1000)
            || !text_valid(&job.instructions, 16384)
            || job.paths.is_empty()
            || job.paths.len() > 64
            || job.context.len() > 64
            || job.depends.len() > 64
        {
            return Err("Invalid or duplicate plan task, instructions or scopes".into());
        }
        if job.paths.iter().collect::<BTreeSet<_>>().len() != job.paths.len()
            || job.context.iter().collect::<BTreeSet<_>>().len() != job.context.len()
        {
            return Err("Duplicate task scope".into());
        }
        for path in job.paths.iter().chain(&job.context) {
            if path != "."
                && (path.contains(['*', '?', '[', ']']) || normalize_path(path)? != *path)
            {
                return Err(
                    "Task scopes must be canonical exact paths or directory prefixes".into(),
                );
            }
            if path != "." {
                // resolve accepts files, whereas task prefixes may name an
                // existing directory. Resolve a child to validate its ancestry.
                if project.root.join(path).is_dir() {
                    project.resolve(&format!("{path}/.aienv-workflow-scope-validation"))?;
                } else {
                    project.resolve(path)?;
                }
            }
        }
    }
    let mut done = BTreeSet::new();
    for job in &plan.tasks {
        let deps = job.depends.iter().collect::<BTreeSet<_>>();
        if deps.len() != job.depends.len()
            || job.depends.iter().any(|d| d == &job.id || !ids.contains(d))
        {
            return Err("Missing, duplicate or self task dependency".into());
        }
    }
    loop {
        let before = done.len();
        for job in &plan.tasks {
            if job.depends.iter().all(|d| done.contains(d)) {
                done.insert(job.id.clone());
            }
        }
        if done.len() == plan.tasks.len() {
            break;
        }
        if before == done.len() {
            return Err("Task dependency cycle".into());
        }
    }
    fn ancestor(tasks: &[Job], reader: &Job, writer: &str, seen: &mut BTreeSet<String>) -> bool {
        reader.depends.iter().any(|dependency| {
            dependency == writer
                || (seen.insert(dependency.clone())
                    && tasks
                        .iter()
                        .find(|task| &task.id == dependency)
                        .is_some_and(|task| ancestor(tasks, task, writer, seen)))
        })
    }
    for reader in &plan.tasks {
        for writer in &plan.tasks {
            if reader.id != writer.id
                && (writer.paths.iter().any(|path| metadata_path(path))
                    || reader.context.iter().any(|read| {
                        writer
                            .paths
                            .iter()
                            .any(|write| scope_contains(read, write) || scope_contains(write, read))
                    }))
                && !ancestor(&plan.tasks, reader, &writer.id, &mut BTreeSet::new())
            {
                return Err(format!(
                    "Task {} reads task {} write scope; declare its provider dependency",
                    reader.id, writer.id
                )
                .into());
            }
        }
    }
    verification::validate_plan(
        project,
        &plan.verification,
        plan.acceptance.len(),
        &verification_scopes(plan),
        required_checks,
    )?;
    Ok(())
}
fn verification_scopes(plan: &Plan) -> Vec<verification::TaskScope<'_>> {
    plan.tasks
        .iter()
        .map(|job| verification::TaskScope {
            id: &job.id,
            paths: &job.paths,
            context: &job.context,
        })
        .collect()
}
fn verification_contract_hash(state: &State) -> Result<String> {
    let plan = state.plan.as_ref().ok_or("Verification requires a plan")?;
    Ok(sha(serde_json::to_vec(&(
        &plan.verification,
        &plan.acceptance,
        &plan.tasks,
        &state.required_checks,
    ))?))
}
/// Validate the full plan contract before the host commits its routing decision.
pub(crate) fn validate_plan_report(
    project: &Project,
    value: &Value,
    required_checks: &[String],
) -> Result<()> {
    let plan: Plan = serde_json::from_value(value.clone())?;
    validate_plan(project, &plan, required_checks)
}
fn load(project: &Project, id: &str) -> Result<State> {
    let state: Option<State> = project.read_state(&filename(id)?)?;
    let state = state.ok_or("Workflow does not exist")?;
    if state.id != id
        || !text_valid(&state.goal, 65536)
        || !hash_valid(&state.snapshot)
        || !matches!(state.mode.as_str(), "auto" | "single" | "team")
        || !(1..=4).contains(&state.workers)
        || !(1..=5).contains(&state.rounds)
        || state.round == 0
        || state.round > state.rounds
        || !matches!(
            state.phase.as_str(),
            "planning" | "developing" | "reviewing" | "qa" | "done" | "blocked"
        )
    {
        return Err("Invalid workflow state".into());
    }
    // Historical plans may reference checks changed externally. Freshness is
    // checked before every transition; status must still explain recovery.
    Ok(state)
}
fn save(project: &Project, state: &State) -> Result<()> {
    project.write_state(&filename(&state.id)?, state)
}
fn fresh(project: &Project, state: &State, report: &str) -> Result<()> {
    if state.snapshot != report || project.snapshot()?.hash != state.snapshot {
        return Err("Stale workflow/source snapshot; use workflow replan before continuing".into());
    }
    Ok(())
}
fn applied(state: &State, id: &str) -> bool {
    state
        .task_states
        .get(id)
        .is_some_and(|task| matches!(task.status.as_str(), "applied" | "verified" | "completed"))
}
fn conflicts(a: &Job, b: &Job) -> bool {
    let overlaps = |left: &[String], right: &[String]| {
        left.iter().any(|a| {
            right
                .iter()
                .any(|b| scope_contains(a, b) || scope_contains(b, a))
        })
    };
    overlaps(&a.paths, &b.paths) || overlaps(&a.paths, &b.context) || overlaps(&b.paths, &a.context)
}
fn architecture_contract_hash(state: &State) -> Result<String> {
    Ok(sha(serde_json::to_vec(
        &state
            .plan
            .as_ref()
            .and_then(|plan| plan.architecture.as_ref()),
    )?))
}
fn architecture_feedback(
    project: &Project,
    state: &State,
    snapshot: &str,
    verdict: &str,
    findings: &[String],
    tasks: &[String],
    report: Option<&ArchitectureReport>,
    required: bool,
) -> Result<(Vec<String>, Vec<String>, Option<StoredArchitectureReport>)> {
    if required && report.is_none() {
        return Err(
            "Architecture contract requires an explicit architecture assessment at this gate"
                .into(),
        );
    }
    let mut merged_findings = findings.to_vec();
    let mut merged_tasks = tasks.iter().cloned().collect::<BTreeSet<_>>();
    // Empty legacy task IDs deliberately mean conservative repair of all tasks.
    // Architecture-only failures instead have precise mandatory issue targets.
    let conservative = !findings.is_empty() && tasks.is_empty();
    let stored = if let Some(report) = report {
        if report.assessment.is_empty()
            || report.assessment.len() > 8
            || report
                .assessment
                .iter()
                .any(|entry| !architecture_text_valid(entry))
            || report.issues.len() > 8
        {
            return Err(
                "Architecture report requires 1..8 bounded assessment entries and at most 8 issues"
                    .into(),
            );
        }
        let plan = state
            .plan
            .as_ref()
            .ok_or("Architecture report requires a plan")?;
        let source = project.snapshot()?;
        if source.hash != snapshot {
            return Err(
                "Architecture report must describe the actual current source snapshot".into(),
            );
        }
        for issue in &report.issues {
            if !matches!(issue.severity.as_str(), "blocking" | "advisory")
                || issue.tasks.is_empty()
                || issue.tasks.len() > 64
                || issue.tasks.iter().collect::<BTreeSet<_>>().len() != issue.tasks.len()
                || issue
                    .tasks
                    .iter()
                    .any(|id| !plan.tasks.iter().any(|task| &task.id == id))
                || [
                    &issue.problem,
                    &issue.consequence,
                    &issue.fix,
                    &issue.verification,
                ]
                .iter()
                .any(|text| !architecture_text_valid(text))
            {
                return Err("Architecture issue needs severity, unique existing task IDs and bounded problem/consequence/fix/verification".into());
            }
            if issue.path.len() > 512 || normalize_path(&issue.path)? != issue.path {
                return Err("Architecture issue location requires a canonical indexed file path up to 512 bytes".into());
            }
            project.resolve(&issue.path)?;
            let file = source
                .files
                .get(&issue.path)
                .ok_or("Architecture issue must reference an indexed source file")?;
            if issue.line == 0 || issue.line > file.text.lines().count() {
                return Err(
                    "Architecture issue line must exist in the referenced source file".into(),
                );
            }
            if !plan.tasks.iter().any(|task| {
                issue.tasks.contains(&task.id)
                    && task
                        .paths
                        .iter()
                        .any(|scope| scope_contains(scope, &issue.path))
            }) {
                return Err(
                    "Architecture issue location is outside its affected tasks' write scopes"
                        .into(),
                );
            }
            if issue.severity == "blocking" {
                if verdict == "accept" {
                    return Err("Blocking architecture issues cannot be accepted".into());
                }
                merged_findings.push(format!(
                    "Architecture {}:{}: {}\nConsequence: {}\nFix: {}\nVerification: {}",
                    issue.path,
                    issue.line,
                    issue.problem,
                    issue.consequence,
                    issue.fix,
                    issue.verification
                ));
                if !conservative {
                    merged_tasks.extend(issue.tasks.iter().cloned());
                }
            }
        }
        Some(StoredArchitectureReport {
            snapshot: snapshot.into(),
            contract_hash: architecture_contract_hash(state)?,
            assessment: report.assessment.clone(),
            issues: report.issues.clone(),
        })
    } else {
        None
    };
    // Keep original validation for native legacy reports, including duplicate
    // task rejection on repairs; merging must not normalize malformed inputs.
    if verdict == "changes" && tasks.iter().collect::<BTreeSet<_>>().len() != tasks.len() {
        return Err("Repair targets must be unique existing task IDs".into());
    }
    // Legacy free-form findings retain their original byte limits. Generated
    // findings have already been bounded field-by-field in Unicode characters;
    // applying the legacy byte cap would reject valid non-ASCII evidence.
    let has_blocker = merged_findings.len() > findings.len();
    findings_valid(
        findings,
        if verdict == "changes" && has_blocker {
            "accept"
        } else {
            verdict
        },
    )?;
    if !matches!(verdict, "accept" | "changes")
        || merged_findings.len() > 64
        || (verdict == "changes" && merged_findings.is_empty())
    {
        return Err("Verdict must be accept or changes; changes requires bounded findings".into());
    }
    Ok((
        merged_findings,
        if conservative {
            vec![]
        } else {
            merged_tasks.into_iter().collect()
        },
        stored,
    ))
}
fn metadata_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    matches!(
        name.as_str(),
        "agents.md"
            | "agents.override.md"
            | ".gitignore"
            | ".eslintignore"
            | "claude.md"
            | ".editorconfig"
            | "rustfmt.toml"
            | ".rustfmt.toml"
            | ".clang-format"
            | ".clang-tidy"
            | "pyproject.toml"
            | "cargo.toml"
            | "package.json"
            | "directory.build.props"
            | "directory.build.targets"
            | "global.json"
            | "biome.json"
            | "biome.jsonc"
            | "ruff.toml"
            | ".ruff.toml"
            | "setup.cfg"
            | "tox.ini"
    ) || name.starts_with(".prettier")
        || name.starts_with("eslint.config.")
        || name.starts_with(".eslintrc")
        || (name.starts_with("tsconfig") && name.ends_with(".json"))
        || name.ends_with(".csproj")
        || name.ends_with(".props")
        || name.ends_with(".targets")
}
fn input_hash(snapshot: &Snapshot, config: &[u8], job: &Job) -> String {
    let scopes = job
        .paths
        .iter()
        .chain(&job.context)
        .cloned()
        .collect::<Vec<_>>();
    let mut bytes = [config, snapshot.scope_hash(&scopes).as_bytes()].concat();
    // Project rules and build/style configuration affect scoped model inputs
    // even when they live outside the task's source prefixes.
    for (path, file) in &snapshot.files {
        if metadata_path(path) {
            bytes.extend_from_slice(path.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(file.hash.as_bytes());
            bytes.push(0);
        }
    }
    sha(bytes)
}
fn ready(state: &State) -> Result<Vec<&Job>> {
    if state.phase != "developing" {
        return Err("Developer jobs are available only in developing phase".into());
    }
    let plan = state.plan.as_ref().ok_or("Workflow has no plan")?;
    let active = state
        .attempts
        .iter()
        .filter(|attempt| matches!(attempt.status.as_str(), "running" | "proposed"))
        .filter_map(|attempt| plan.tasks.iter().find(|job| job.id == attempt.task))
        .collect::<Vec<_>>();
    let capacity = state.workers.saturating_sub(active.len());
    let mut selected: Vec<&Job> = Vec::new();
    for job in &plan.tasks {
        if selected.len() == capacity {
            break;
        }
        if applied(state, &job.id)
            || active.iter().any(|other| other.id == job.id)
            || job.depends.iter().any(|d| !applied(state, d))
        {
            continue;
        }
        if !active
            .iter()
            .chain(selected.iter())
            .any(|other| conflicts(job, other))
        {
            selected.push(job);
        }
    }
    Ok(selected)
}
fn status(project: &Project, state: &State) -> Result<String> {
    let current = project.snapshot()?.hash;
    let plan = state.plan.as_ref();
    let architecture_fresh = state.last_architecture.as_ref().is_some_and(|report| {
        report.snapshot == current
            && architecture_contract_hash(state).is_ok_and(|hash| hash == report.contract_hash)
    });
    let verification_fresh = state.last_verification.as_ref().is_some_and(|report| {
        report.snapshot == current
            && verification_contract_hash(state).is_ok_and(|hash| hash == report.contract_hash)
    });
    json(
        &value!({"id":state.id,"goal":state.goal,"phase":state.phase,"snapshot":state.snapshot,
        "current_snapshot":current,"stale":current != state.snapshot,"workers":state.workers,"mode":state.mode,
        "round":state.round,"rounds":state.rounds,"tasks":plan.map(|p| &p.tasks),
        "checks":plan.map(|p| &p.checks),"acceptance":plan.map(|p| &p.acceptance),
        "architecture":plan.and_then(|p| p.architecture.as_ref()),"architecture_report":state.last_architecture,"architecture_report_fresh":architecture_fresh,
        "verification":plan.map(|p| &p.verification),"verification_report":state.last_verification,"verification_report_fresh":verification_fresh,
        "verification_execution":if state.required_checks.is_empty() {"static_only"} else {"approved_checks"},
        "findings":state.findings,"completed":state.completed,"summaries":state.summaries,
        "task_states":state.task_states,"attempts":state.attempts,"pending_apply":state.pending,"assessment":state.assessment,"failed_proofs":state.failed_proofs,
        "proofs":state.proofs,"review_snapshot":state.review_snapshot,"required_checks":state.required_checks,
        "automated_checks":if state.required_checks.is_empty() {"none_configured"} else {"configured"}}),
    )
}
fn read_bytes(path: &str, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("Workflow input exceeds limit".into());
    }
    Ok(bytes)
}
fn report<T: DeserializeOwned>(args: &[String]) -> Result<T> {
    if args.len() != 2 || args[0] != "--file" {
        return Err("Expected --file REPORT".into());
    }
    Ok(serde_json::from_slice(&read_bytes(&args[1], MAX_REPORT)?)?)
}
fn findings_valid(findings: &[String], verdict: &str) -> Result<()> {
    if !matches!(verdict, "accept" | "changes")
        || findings.len() > 64
        || findings.iter().any(|f| !text_valid(f, 4096))
        || (verdict == "changes" && findings.is_empty())
    {
        return Err("Verdict must be accept or changes; changes requires bounded findings".into());
    }
    Ok(())
}
fn invalidated(state: &State, targets: &[String]) -> Result<BTreeSet<String>> {
    let plan = state.plan.as_ref().ok_or("Workflow has no plan")?;
    let mut selected = if targets.is_empty() {
        plan.tasks
            .iter()
            .map(|job| job.id.clone())
            .collect::<BTreeSet<_>>()
    } else {
        targets.iter().cloned().collect()
    };
    if selected.len() != targets.len() && !targets.is_empty()
        || selected
            .iter()
            .any(|id| !plan.tasks.iter().any(|job| &job.id == id))
    {
        return Err("Repair targets must be unique existing task IDs".into());
    }
    loop {
        let before = selected.len();
        for job in &plan.tasks {
            if job.depends.iter().any(|id| selected.contains(id)) {
                selected.insert(job.id.clone());
            }
        }
        if before == selected.len() {
            return Ok(selected);
        }
    }
}
fn invalidate(state: &mut State, targets: &BTreeSet<String>, findings: &[String]) {
    for progress in state.task_states.values_mut() {
        if progress.status == "verified" {
            progress.status = "applied".into();
        }
    }
    state.assessment.clear();
    for id in targets {
        let progress = state.task_states.entry(id.clone()).or_default();
        progress.status = "pending".into();
        progress.generation += 1;
        progress.input = None;
        progress.findings = findings.to_vec();
        state.completed.remove(id);
        // Historical summaries remain useful context; they do not indicate completion.
    }
    for attempt in &mut state.attempts {
        if targets.contains(&attempt.task)
            && matches!(attempt.status.as_str(), "running" | "proposed")
        {
            attempt.status = "superseded".into();
        }
    }
    state.review_snapshot = None;
    state.proofs.clear();
}
fn repair(
    state: &mut State,
    snapshot: String,
    findings: Vec<String>,
    targets: &[String],
    failure_identity: Option<String>,
) -> Result<()> {
    let affected = invalidated(state, targets)?;
    let identity = failure_identity.unwrap_or(sha(serde_json::to_vec(&findings)?));
    let signature = sha(serde_json::to_vec(&(
        snapshot.clone(),
        identity,
        &affected,
    ))?);
    let repeated = !state.failures.insert(signature);
    state.snapshot = snapshot;
    state.findings = findings.clone();
    invalidate(state, &affected, &findings);
    if repeated || state.round == state.rounds {
        state.phase = "blocked".into();
        if repeated {
            state.findings.push(
                "No progress: repeated diagnostic on unchanged source; revise the approach".into(),
            );
        }
    } else {
        state.round += 1;
        state.phase = "developing".into();
    }
    Ok(())
}
fn proposal_name(workflow: &str, attempt: &str) -> String {
    format!("workflows/proposals/{workflow}/{attempt}.json")
}
fn next_phase(state: &mut State) {
    if state
        .plan
        .as_ref()
        .is_some_and(|plan| plan.tasks.iter().all(|job| applied(state, &job.id)))
    {
        state.phase = if state.mode == "single" {
            "qa"
        } else {
            "reviewing"
        }
        .into();
        if state.mode == "single" {
            state.review_snapshot = Some(state.snapshot.clone());
        }
    }
}
fn finish_apply(project: &Project, state: &mut State, pending: Pending) -> Result<()> {
    let snapshot = project.snapshot()?;
    let config = fs::read(project.state.join("config.json"))?;
    state.snapshot = snapshot.hash.clone();
    for changes in pending.jobs {
        let job = state
            .plan
            .as_ref()
            .and_then(|plan| plan.tasks.iter().find(|job| job.id == changes.id))
            .ok_or("Pending apply refers to missing task")?;
        let input = input_hash(&snapshot, &config, job);
        let progress = state
            .task_states
            .get_mut(&changes.id)
            .ok_or("Missing task progress")?;
        progress.status = "applied".into();
        progress.input = Some(input);
        state.summaries.insert(changes.id, changes.summary);
    }
    for attempt in &mut state.attempts {
        if pending.attempts.contains(&attempt.id) {
            attempt.status = "applied".into();
        }
    }
    // A later authorized task can refine a file written by its dependency.
    // Retain the last integrated baseline for applied work; external replan
    // compares against this baseline rather than a pre-integration intermediate.
    if let Some(plan) = &state.plan {
        for job in &plan.tasks {
            if let Some(progress) = state.task_states.get_mut(&job.id) {
                if matches!(progress.status.as_str(), "applied" | "verified") {
                    progress.input = Some(input_hash(&snapshot, &config, job));
                }
            }
        }
    }
    state.pending = None;
    next_phase(state);
    Ok(())
}
fn reconcile(project: &Project, state: &mut State) -> Result<()> {
    if let Some(pending) = state.pending.clone() {
        let current = project.snapshot()?.hash;
        if current == pending.after {
            finish_apply(project, state, pending)?;
        } else if current == pending.before {
            state.pending = None;
        } else {
            state.phase = "blocked".into();
            state.findings.push("Interrupted apply has ambiguous source state; inspect the saved apply intent before recovery".into());
        }
        save(project, state)?;
    }
    Ok(())
}
/// Resolve all replacements against virtual content before the native transaction.
/// Each resulting file has one original CAS hash and one final source, so a late
/// replacement failure cannot leave earlier replacements on disk.
fn materialize(project: &Project, snapshot: &Snapshot, edits: &[Value]) -> Result<Vec<Value>> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for edit in edits {
        let edit: NativeEdit = serde_json::from_value(edit.clone())?;
        let path = edit.path.as_str();
        if normalize_path(path)? != path || !seen.insert(path.to_owned()) {
            return Err("Edits require unique canonical paths; group replacements per file".into());
        }
        if path.rsplit('/').next() == Some(".gitignore") {
            return Err("Workflow edits cannot change Git index discovery rules; update them separately and replan".into());
        }
        let destination = project.resolve(path)?;
        if !snapshot.files.contains_key(path) {
            project.allows_new(path)?;
            match fs::symlink_metadata(destination) {
                Ok(_) => return Err(format!("Existing file is not indexed: {path}").into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        let before = snapshot.files.get(path);
        let expected = &edit.expected_hash;
        if expected.as_str() != before.map(|file| file.hash.as_str())
            || (before.is_none() && !expected.is_null())
        {
            return Err(format!("Stale or missing expected_hash: {path}").into());
        }
        let source = match (edit.source, edit.old, edit.new, edit.replacements) {
            (Some(source), None, None, None) => source,
            (None, old, new, replacements) => {
                let replacements = match (old, new, replacements) {
                    (Some(old), Some(new), None) => vec![Replacement { old, new }],
                    (None, None, Some(replacements))
                        if !replacements.is_empty() && replacements.len() <= 128 =>
                    {
                        replacements
                    }
                    _ => {
                        return Err(
                            "Use either source, both old and new, or 1..128 replacements".into(),
                        )
                    }
                };
                let mut text = before.ok_or("New file requires source")?.text.clone();
                for replacement in replacements {
                    let old = replacement.old;
                    let new = replacement.new;
                    if old.is_empty()
                        || text.find(&old).is_none()
                        || text.find(&old) != text.rfind(&old)
                    {
                        return Err(format!(
                            "old must match exactly once in current virtual content: {path}"
                        )
                        .into());
                    }
                    let size = text.len() - old.len();
                    if new.len() > 2 * 1024 * 1024 - size || new.contains('\0') {
                        return Err("Invalid replacement source size or NUL".into());
                    }
                    text = text.replacen(&old, &new, 1);
                }
                text
            }
            _ => return Err("Use either source, both old and new, or replacements".into()),
        };
        if source.len() > 2 * 1024 * 1024 || source.contains('\0') {
            return Err("Invalid source size or NUL".into());
        }
        result.push(value!({"path":path,"expected_hash":expected,"source":source}));
    }
    Ok(result)
}
fn predicted(project: &Project, snapshot: &Snapshot, edits: &[Value]) -> Result<String> {
    let mut hashes = snapshot
        .files
        .iter()
        .map(|(path, file)| (path.clone(), file.hash.clone()))
        .collect::<BTreeMap<_, _>>();
    for edit in edits {
        hashes.insert(
            edit["path"].as_str().ok_or("Edit needs path")?.into(),
            sha(edit["source"]
                .as_str()
                .ok_or("Materialized edit needs source")?
                .as_bytes()),
        );
    }
    let mut digest = Sha256::new();
    digest.update(b"ai-env-snapshot-1\0");
    digest.update(fs::read(project.state.join("config.json"))?);
    for (path, hash) in hashes {
        digest.update(path.as_bytes());
        digest.update([0]);
        digest.update(hash.as_bytes());
        digest.update([0]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
fn refresh_proposals(project: &Project, state: &mut State) -> Result<()> {
    let snapshot = project.snapshot()?;
    let config = fs::read(project.state.join("config.json"))?;
    for index in 0..state.attempts.len() {
        let attempt = &state.attempts[index];
        if attempt.status != "proposed" {
            continue;
        }
        let job = state
            .plan
            .as_ref()
            .and_then(|plan| plan.tasks.iter().find(|job| job.id == attempt.task));
        let valid = job.is_some_and(|job| {
            state
                .task_states
                .get(&job.id)
                .is_some_and(|progress| progress.generation == attempt.generation)
                && attempt.inputs == input_hash(&snapshot, &config, job)
        });
        if !valid {
            let task = attempt.task.clone();
            let generation = attempt.generation;
            state.attempts[index].status = "superseded".into();
            state.attempts[index].error =
                Some("Saved proposal input changed; request a fresh scoped attempt".into());
            if let Some(progress) = state.task_states.get_mut(&task) {
                if progress.generation == generation {
                    progress.status = "pending".into();
                    progress.findings = vec!["Saved proposal input changed, including project rules or style configuration; re-evaluate against current inputs".into()];
                }
            }
        }
    }
    Ok(())
}
fn validate_proofs(
    project: &Project,
    state: &State,
    snapshot: &str,
    proofs: &[String],
) -> Result<()> {
    fresh(project, state, snapshot)?;
    if state.phase != "qa" || state.review_snapshot.as_deref() != Some(snapshot) {
        return Err("Verification requires QA of the accepted source snapshot".into());
    }
    if proofs.len() > 64 {
        return Err("Too many verification proofs".into());
    }
    let config = project.config()?;
    let config_hash = sha(fs::read(project.state.join("config.json"))?);
    let mut verified = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for id in proofs {
        if !ids.insert(id) {
            return Err("Duplicate verification proof".into());
        }
        let proof = verify::load(project, id)?;
        let check = config
            .checks
            .get(&proof.check)
            .ok_or("Proof refers to unconfigured check")?;
        if !state.required_checks.contains(&proof.check)
            || !proof.passed
            || proof.exit_code != Some(0)
            || proof.snapshot != snapshot
            || proof.snapshot_after.as_deref() != Some(snapshot)
            || proof.config_hash != config_hash
            || proof.argv != check.argv
            || !proof.source_unchanged
            || proof.timed_out
            || proof.output_limited
            || !proof.capture_complete
            || proof.execution_error.is_some()
        {
            return Err(
                "Verification requires fresh passing actual source/configuration proofs".into(),
            );
        }
        verified.insert(proof.check);
    }
    if state
        .required_checks
        .iter()
        .any(|check| !verified.contains(check))
    {
        return Err("Verification lacks a passing proof for an approved check".into());
    }
    fresh(project, state, snapshot)
}
fn default_check(name: &str) -> bool {
    name.to_ascii_lowercase().split(['-', '_']).any(|token| {
        matches!(
            token,
            "test"
                | "tests"
                | "unit"
                | "integration"
                | "qa"
                | "check"
                | "checks"
                | "lint"
                | "style"
        )
    })
}

pub fn run(project: &Project, args: &[String]) -> Result<String> {
    let action = args.first().ok_or("Expected workflow action")?;
    if action == "list" {
        if args.len() != 1 {
            return Err("workflow list accepts no options".into());
        }
        let directory = project.state.join("workflows");
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err("Invalid workflow state directory".into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut ids = Vec::new();
        match fs::read_dir(directory) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry?;
                    if let Some(id) = entry
                        .file_name()
                        .to_str()
                        .and_then(|name| name.strip_suffix(".json"))
                    {
                        if valid_id(id) {
                            ids.push(id.to_owned());
                        }
                    }
                    if ids.len() > 1000 {
                        return Err("Workflow listing exceeds 1000 states".into());
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        ids.sort();
        let mut summaries = Vec::new();
        let snapshot = project.snapshot()?.hash;
        for id in ids {
            let state = load(project, &id)?;
            let tasks = state
                .task_states
                .iter()
                .map(|(id, progress)| (id.clone(), value!({"status":progress.status})))
                .collect::<BTreeMap<_, _>>();
            let goal = state.goal.chars().take(160).collect::<String>();
            summaries.push(value!({"id":id,"goal":goal,"goal_truncated":state.goal.chars().count()>160,"mode":state.mode,"phase":state.phase,
                "snapshot":state.snapshot,"stale":state.snapshot != snapshot,"task_states":tasks,
                "attempts":state.attempts.len(),"proofs":state.proofs.len(),"required_checks":state.required_checks}));
        }
        summaries.sort_by(|a, b| {
            let terminal =
                |value: &Value| matches!(value["phase"].as_str(), Some("done" | "blocked"));
            terminal(a)
                .cmp(&terminal(b))
                .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
        });
        let omitted = summaries.len().saturating_sub(32);
        summaries.truncate(32);
        return json(&value!({"workflows":summaries,"omitted":omitted}));
    }
    if action == "checks" {
        if args.len() != 1 {
            return Err("workflow checks accepts no options".into());
        }
        let config = project.config()?;
        let available = config.checks.keys().collect::<Vec<_>>();
        let defaults = available
            .iter()
            .filter(|name| default_check(name))
            .collect::<Vec<_>>();
        return json(&value!({"available":available,"defaults":defaults}));
    }
    let id = args.get(1).ok_or("Expected workflow ID")?;
    let name = filename(id)?;
    let tail = &args[2..];
    if action == "start" {
        let exists: Option<State> = project.read_state(&name)?;
        if exists.is_some() {
            return Err("Workflow already exists".into());
        }
        let mut options = BTreeMap::new();
        let mut checks = Vec::new();
        for pair in tail.chunks(2) {
            if pair.len() == 2 && pair[0] == "--check" {
                checks.push(pair[1].clone());
                continue;
            }
            if pair.len() != 2
                || !matches!(
                    pair[0].as_str(),
                    "--request" | "--workers" | "--rounds" | "--mode"
                )
                || options.insert(pair[0].as_str(), pair[1].as_str()).is_some()
            {
                return Err(
                    "Usage: workflow start ID --request FILE [--workers 1..4 --rounds 1..5]".into(),
                );
            }
        }
        let request = options.get("--request").ok_or("Expected --request FILE")?;
        let goal = String::from_utf8(read_bytes(request, 65536)?)?;
        let workers: usize = options.get("--workers").unwrap_or(&"2").parse()?;
        let rounds: usize = options.get("--rounds").unwrap_or(&"2").parse()?;
        let mode = options.get("--mode").unwrap_or(&"team").to_string();
        if !matches!(mode.as_str(), "auto" | "single" | "team") {
            return Err("Invalid workflow mode".into());
        }
        let workers = if mode == "single" { 1 } else { workers };
        if !text_valid(&goal, 65536) || !(1..=4).contains(&workers) || !(1..=5).contains(&rounds) {
            return Err("Invalid goal, workers or rounds".into());
        }
        let configured = project.config()?.checks;
        if checks.is_empty() {
            checks = configured
                .keys()
                .filter(|name| default_check(name))
                .cloned()
                .collect();
        }
        if checks.len() > 64
            || checks.iter().collect::<BTreeSet<_>>().len() != checks.len()
            || checks.iter().any(|c| !configured.contains_key(c))
        {
            return Err("Checks must be unique configured check names".into());
        }
        let state = State {
            id: id.clone(),
            goal,
            phase: "planning".into(),
            mode,
            snapshot: project.snapshot()?.hash,
            workers,
            round: 1,
            rounds,
            required_checks: checks,
            plan: None,
            completed: BTreeMap::new(),
            summaries: BTreeMap::new(),
            findings: Vec::new(),
            review_snapshot: None,
            proofs: Vec::new(),
            task_states: BTreeMap::new(),
            attempts: Vec::new(),
            pending: None,
            failures: BTreeSet::new(),
            assessment: Vec::new(),
            failed_proofs: Vec::new(),
            last_architecture: None,
            last_verification: None,
        };
        save(project, &state)?;
        return status(project, &state);
    }
    let mut state = load(project, id)?;
    if action != "status" {
        reconcile(project, &mut state)?;
    }
    match action.as_str() {
        "status" if tail.is_empty() => return status(project, &state),
        "ready" if tail.is_empty() => {
            fresh(project, &state, &state.snapshot)?;
            return json(&value!({"snapshot":state.snapshot,"jobs":ready(&state)?}));
        }
        "route" => {
            let route: Value = report(tail)?;
            let mode = route["mode"].as_str().ok_or("Route needs mode")?;
            if state.phase != "planning"
                || state.plan.is_some()
                || !matches!(mode, "single" | "team")
            {
                return Err("Route needs planning without a plan, and single or team mode".into());
            }
            state.mode = mode.into();
            if mode == "single" {
                state.workers = 1;
            }
        }
        "replan" if tail.is_empty() => {
            if matches!(state.phase.as_str(), "done" | "blocked") {
                return Err("Terminal workflow cannot be replanned; start a new workflow".into());
            }
            let snapshot = project.snapshot()?;
            let config = fs::read(project.state.join("config.json"))?;
            let changed = state
                .plan
                .as_ref()
                .map(|plan| {
                    plan.tasks
                        .iter()
                        .filter(|job| {
                            let previous = state
                                .task_states
                                .get(&job.id)
                                .and_then(|progress| progress.input.as_ref())
                                .or_else(|| {
                                    state
                                        .attempts
                                        .iter()
                                        .rev()
                                        .find(|attempt| {
                                            attempt.task == job.id
                                                && matches!(
                                                    attempt.status.as_str(),
                                                    "running" | "proposed"
                                                )
                                        })
                                        .map(|attempt| &attempt.inputs)
                                });
                            previous.is_none_or(|old| old != &input_hash(&snapshot, &config, job))
                        })
                        .map(|job| job.id.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if state.plan.is_some() {
                let affected = if changed.is_empty() {
                    BTreeSet::new()
                } else {
                    invalidated(&state, &changed)?
                };
                invalidate(
                    &mut state,
                    &affected,
                    &["Source or plan changed; re-evaluate this task".into()],
                );
            }
            state.snapshot = snapshot.hash;
            state.findings = vec![
                "Explicit replan; unchanged task contracts and scoped results remain available"
                    .into(),
            ];
            state.review_snapshot = None;
            state.proofs.clear();
            if state.round == state.rounds {
                state.phase = "blocked".into();
            } else {
                state.round += 1;
                state.phase = "planning".into();
            }
        }
        "plan" | "single" => {
            if state.phase != "planning" {
                return Err("Architect plan requires planning phase".into());
            }
            let plan: Plan = report(tail)?;
            fresh(project, &state, &plan.snapshot)?;
            validate_plan(project, &plan, &state.required_checks)?;
            if action == "single" {
                state.mode = "single".into();
                state.workers = 1;
            }
            if state.mode == "auto" {
                return Err("Route auto workflow before submitting a plan".into());
            }
            if state.mode == "single" && plan.tasks.len() != 1 {
                return Err("Single mode requires exactly one task".into());
            }
            let changed = plan
                .tasks
                .iter()
                .filter(|job| {
                    state
                        .plan
                        .as_ref()
                        .and_then(|old| old.tasks.iter().find(|old| old.id == job.id))
                        != Some(*job)
                })
                .map(|job| job.id.clone())
                .collect::<Vec<_>>();
            let mut changed = changed.into_iter().collect::<BTreeSet<_>>();
            if let Some(old) = &state.plan {
                // Architecture is a global contract. A changed contract may
                // alter any task's applicability even when its text is identical.
                if old.architecture != plan.architecture {
                    changed.extend(plan.tasks.iter().map(|job| job.id.clone()));
                }
                changed.extend(
                    old.tasks
                        .iter()
                        .filter(|old| !plan.tasks.iter().any(|job| job.id == old.id))
                        .map(|job| job.id.clone()),
                );
            }
            loop {
                let before = changed.len();
                for job in &plan.tasks {
                    if job.depends.iter().any(|id| changed.contains(id)) {
                        changed.insert(job.id.clone());
                    }
                }
                if before == changed.len() {
                    break;
                }
            }
            invalidate(&mut state, &changed, &[]);
            state
                .task_states
                .retain(|id, _| plan.tasks.iter().any(|job| &job.id == id));
            state
                .completed
                .retain(|id, _| plan.tasks.iter().any(|job| &job.id == id));
            for job in &plan.tasks {
                state
                    .task_states
                    .entry(job.id.clone())
                    .or_insert_with(|| TaskProgress {
                        status: "pending".into(),
                        ..TaskProgress::default()
                    });
            }
            state.plan = Some(plan);
            state.phase = "developing".into();
            next_phase(&mut state);
        }
        "attempt-start" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Reserve {
                task: String,
                owner: String,
            }
            let reserve: Reserve = report(tail)?;
            fresh(project, &state, &state.snapshot)?;
            if !text_valid(&reserve.owner, 256) || state.attempts.len() >= 256 {
                return Err("Invalid owner or workflow attempt limit reached".into());
            }
            let job = ready(&state)?
                .into_iter()
                .find(|job| job.id == reserve.task)
                .ok_or("Task is not ready for an attempt")?
                .clone();
            let snapshot = project.snapshot()?;
            let inputs = input_hash(
                &snapshot,
                &fs::read(project.state.join("config.json"))?,
                &job,
            );
            let attempt = Attempt {
                id: format!("attempt-{}", state.attempts.len() + 1),
                task: job.id.clone(),
                owner: reserve.owner,
                snapshot: snapshot.hash,
                inputs,
                generation: state.task_states[&job.id].generation,
                status: "running".into(),
                error: None,
                summary: None,
            };
            state
                .task_states
                .get_mut(&job.id)
                .ok_or("Missing task progress")?
                .status = "running".into();
            state.attempts.push(attempt.clone());
            save(project, &state)?;
            return json(&value!({"attempt":attempt}));
        }
        "attempt-status" => {
            if tail.len() != 2 || tail[0] != "--attempt" {
                return Err("Expected --attempt ID".into());
            }
            let attempt = state
                .attempts
                .iter()
                .find(|attempt| attempt.id == tail[1])
                .ok_or("Unknown attempt")?;
            let proposal: Option<Changes> = project.read_state(&proposal_name(id, &attempt.id))?;
            return json(&value!({"attempt":attempt,"proposal":proposal}));
        }
        "attempt-propose" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Proposal {
                attempt: String,
                snapshot: String,
                summary: String,
                edits: Vec<Value>,
            }
            let proposal: Proposal = report(tail)?;
            fresh(project, &state, &state.snapshot)?;
            let index = state
                .attempts
                .iter()
                .position(|attempt| attempt.id == proposal.attempt)
                .ok_or("Unknown attempt")?;
            let attempt = &state.attempts[index];
            // A sibling exhausting its retry budget blocks new work and applies,
            // but an already-running worker may still return a valid result.
            // Preserve that result without reopening or advancing the workflow.
            if !matches!(state.phase.as_str(), "developing" | "blocked")
                || attempt.status != "running"
                || attempt.snapshot != proposal.snapshot
            {
                return Err(
                    "Proposal requires a running attempt and its original input snapshot".into(),
                );
            }
            let job = state
                .plan
                .as_ref()
                .and_then(|plan| plan.tasks.iter().find(|job| job.id == attempt.task))
                .ok_or("Missing attempt task")?;
            let snapshot = project.snapshot()?;
            if input_hash(
                &snapshot,
                &fs::read(project.state.join("config.json"))?,
                job,
            ) != attempt.inputs
                || state.task_states[&job.id].generation != attempt.generation
            {
                return Err(
                    "Attempt inputs changed; supersede it and request a fresh attempt".into(),
                );
            }
            if !text_valid(&proposal.summary, 4096) || proposal.edits.len() > 128 {
                return Err("Invalid attempt summary or edit count".into());
            }
            let mut paths = BTreeSet::new();
            for edit in &proposal.edits {
                let path = edit["path"].as_str().ok_or("Edit needs path")?;
                if normalize_path(path)? != path
                    || !job.paths.iter().any(|scope| scope_contains(scope, path))
                    || !paths.insert(path)
                {
                    return Err(
                        "Proposal edits must have unique paths inside the assigned task scopes"
                            .into(),
                    );
                }
                let _: NativeEdit = serde_json::from_value(edit.clone())?;
            }
            crate::tasks::check_claims(
                project,
                None,
                None,
                &proposal
                    .edits
                    .iter()
                    .filter_map(|edit| edit["path"].as_str().map(str::to_owned))
                    .collect::<Vec<_>>(),
            )?;
            materialize(project, &snapshot, &proposal.edits)?;
            let changes = Changes {
                id: attempt.task.clone(),
                summary: proposal.summary.clone(),
                files: proposal.edits,
            };
            // This atomic durable write precedes the queue transition. Recovery
            // finds it even when the process dies before the state write.
            if serde_json::to_vec_pretty(&changes)?.len() > 7 * 1024 * 1024 {
                return Err("Durable attempt proposal exceeds 7 MiB limit; split the task".into());
            }
            project.write_state(&proposal_name(id, &proposal.attempt), &changes)?;
            fresh(project, &state, &state.snapshot)?;
            state.attempts[index].status = "proposed".into();
            state.attempts[index].summary = Some(proposal.summary);
            state
                .task_states
                .get_mut(&changes.id)
                .ok_or("Missing task progress")?
                .status = "proposed".into();
        }
        "attempt-fail" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Failure {
                attempt: String,
                error: String,
            }
            let failure: Failure = report(tail)?;
            if !text_valid(&failure.error, 16384) {
                return Err("Failure needs bounded diagnostic".into());
            }
            let index = state
                .attempts
                .iter()
                .position(|attempt| attempt.id == failure.attempt)
                .ok_or("Unknown attempt")?;
            let attempt = &state.attempts[index];
            if !matches!(attempt.status.as_str(), "running" | "proposed") {
                return Err("Only an active attempt can fail".into());
            }
            let signature = sha(serde_json::to_vec(&(
                &attempt.task,
                &attempt.inputs,
                &failure.error,
            ))?);
            let repeated = !state.failures.insert(signature);
            let task = attempt.task.clone();
            let generation = attempt.generation;
            state.attempts[index].status = "failed".into();
            state.attempts[index].error = Some(failure.error.clone());
            let failures = state
                .attempts
                .iter()
                .filter(|attempt| {
                    attempt.task == task
                        && attempt.generation == generation
                        && attempt.status == "failed"
                })
                .count();
            let progress = state
                .task_states
                .get_mut(&task)
                .ok_or("Missing task progress")?;
            progress.status = "pending".into();
            progress.findings = vec![failure.error.clone()];
            state.findings = vec![format!("Task {task}: {}", failure.error)];
            if repeated || failures >= state.rounds {
                state.phase = "blocked".into();
                state.findings.push(
                    if repeated {
                        "No progress: repeated failure with unchanged inputs; revise the approach"
                    } else {
                        "Task retry budget exhausted"
                    }
                    .into(),
                );
            }
        }
        "attempt-refresh" if tail.is_empty() => {
            refresh_proposals(project, &mut state)?;
        }
        "attempt-recover" if tail.is_empty() => {
            for index in 0..state.attempts.len() {
                if state.attempts[index].status != "running" {
                    continue;
                }
                let attempt = &state.attempts[index];
                let proposal: Option<Changes> =
                    project.read_state(&proposal_name(id, &attempt.id))?;
                let task = attempt.task.clone();
                let current = state
                    .task_states
                    .get(&task)
                    .is_some_and(|progress| progress.generation == attempt.generation);
                if let Some(proposal) = proposal.filter(|proposal| proposal.id == task && current) {
                    state.attempts[index].status = "proposed".into();
                    state.attempts[index].summary = Some(proposal.summary);
                    state
                        .task_states
                        .get_mut(&task)
                        .ok_or("Missing task progress")?
                        .status = "proposed".into();
                } else {
                    state.attempts[index].status = "interrupted".into();
                    state.attempts[index].error =
                        Some("Executor session was interrupted before a durable result".into());
                    if current {
                        state
                            .task_states
                            .get_mut(&task)
                            .ok_or("Missing task progress")?
                            .status = "pending".into();
                    }
                }
            }
            refresh_proposals(project, &mut state)?;
        }
        "apply" | "preview" | "attempt-apply" | "attempt-preview" => {
            let mut applying_attempts = Vec::new();
            let batch: Batch = if matches!(action.as_str(), "attempt-apply" | "attempt-preview") {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct ApplyAttempts {
                    attempts: Vec<String>,
                }
                let requested: ApplyAttempts = report(tail)?;
                if requested.attempts.iter().collect::<BTreeSet<_>>().len()
                    != requested.attempts.len()
                {
                    return Err("Duplicate attempt ID".into());
                }
                let snapshot = project.snapshot()?;
                let config = fs::read(project.state.join("config.json"))?;
                let mut jobs = Vec::new();
                for attempt_id in &requested.attempts {
                    let attempt = state
                        .attempts
                        .iter()
                        .find(|attempt| &attempt.id == attempt_id)
                        .ok_or("Unknown attempt")?;
                    let job = state
                        .plan
                        .as_ref()
                        .and_then(|plan| plan.tasks.iter().find(|job| job.id == attempt.task))
                        .ok_or("Missing attempt task")?;
                    if attempt.status != "proposed"
                        || state.task_states[&job.id].generation != attempt.generation
                        || input_hash(&snapshot, &config, job) != attempt.inputs
                        || job.depends.iter().any(|id| !applied(&state, id))
                    {
                        return Err(
                            "Saved proposal is stale or not eligible; repair the affected task"
                                .into(),
                        );
                    }
                    let changes: Option<Changes> =
                        project.read_state(&proposal_name(id, attempt_id))?;
                    let changes = changes.ok_or("Missing durable proposal")?;
                    if changes.id != attempt.task {
                        return Err(
                            "Saved proposal task does not match its reserved attempt".into()
                        );
                    }
                    jobs.push(changes);
                }
                applying_attempts = requested.attempts;
                Batch {
                    snapshot: snapshot.hash,
                    jobs,
                }
            } else {
                report(tail)?
            };
            fresh(project, &state, &batch.snapshot)?;
            if batch.jobs.is_empty() || batch.jobs.len() > state.workers {
                return Err("Batch must contain 1..workers ready jobs".into());
            }
            if state.phase != "developing" {
                return Err("Apply requires developing phase".into());
            }
            let eligible = if matches!(action.as_str(), "attempt-apply" | "attempt-preview") {
                state
                    .plan
                    .as_ref()
                    .ok_or("Missing plan")?
                    .tasks
                    .iter()
                    .map(|job| (job.id.clone(), job.clone()))
                    .collect::<BTreeMap<_, _>>()
            } else {
                ready(&state)?
                    .into_iter()
                    .map(|job| (job.id.clone(), job.clone()))
                    .collect::<BTreeMap<_, _>>()
            };
            let mut ids = BTreeSet::new();
            let mut files = BTreeSet::new();
            let mut edits = Vec::new();
            for changes in &batch.jobs {
                let job = eligible
                    .get(&changes.id)
                    .ok_or("Batch contains a job not selected by workflow ready")?;
                if !ids.insert(&changes.id) || !text_valid(&changes.summary, 4096) {
                    return Err("Duplicate job or missing summary".into());
                }
                for file in &changes.files {
                    let path = file
                        .get("path")
                        .and_then(Value::as_str)
                        .ok_or("Each file edit needs a path")?;
                    if normalize_path(path)? != path
                        || !job.paths.iter().any(|s| scope_contains(s, path))
                    {
                        return Err("Developer edit is outside assigned job scopes".into());
                    }
                    if !files.insert(path.to_owned()) {
                        return Err("Duplicate file across developer jobs".into());
                    }
                    edits.push(file.clone());
                }
            }
            if edits.len() > 128 {
                return Err("Batch exceeds 128 native file edits".into());
            }
            let snapshot = project.snapshot()?;
            let edits = materialize(project, &snapshot, &edits)?;
            if matches!(action.as_str(), "preview" | "attempt-preview") {
                if snapshot.hash != batch.snapshot {
                    return Err("Stale workflow/source snapshot during preview".into());
                }
                let mut originals = BTreeMap::new();
                for file in &edits {
                    let path = file["path"].as_str().ok_or("Each file edit needs a path")?;
                    originals.insert(
                        path.to_owned(),
                        snapshot.files.get(path).map(|entry| entry.text.clone()),
                    );
                }
                fresh(project, &state, &batch.snapshot)?;
                return json(&value!({"snapshot":snapshot.hash,"originals":originals}));
            }
            // Persist intent before the native CAS transaction. Recovery can
            // acknowledge exactly the before/after snapshot, never replay blindly.
            let after = predicted(project, &snapshot, &edits)?;
            let pending = Pending {
                before: snapshot.hash.clone(),
                after: after.clone(),
                jobs: batch.jobs,
                attempts: applying_attempts,
            };
            state.pending = Some(pending.clone());
            save(project, &state)?;
            if !edits.is_empty() {
                let patch_name = format!("workflows/patches/{id}.json");
                project.write_state(
                    &patch_name,
                    &value!({"snapshot":batch.snapshot,"edits":edits}),
                )?;
                let patch_path = project.state.join(&patch_name);
                let result = edit::run(
                    project,
                    &["--patch".into(), patch_path.to_string_lossy().into_owned()],
                );
                let _ = fs::remove_file(&patch_path);
                if let Err(error) = result {
                    if project.snapshot()?.hash == pending.before {
                        state.pending = None;
                        save(project, &state)?;
                    } else {
                        reconcile(project, &mut state)?;
                    }
                    return Err(error);
                }
            }
            if project.snapshot()?.hash != after {
                reconcile(project, &mut state)?;
                return Err("Source changed during apply; saved intent requires inspection".into());
            }
            finish_apply(project, &mut state, pending)?;
        }
        "review" => {
            if state.phase != "reviewing" {
                return Err("Review requires completed developer jobs".into());
            }
            let review: Review = report(tail)?;
            fresh(project, &state, &review.snapshot)?;
            let required_architecture = state
                .plan
                .as_ref()
                .is_some_and(|plan| plan.architecture.is_some());
            let (findings, tasks, architecture) = architecture_feedback(
                project,
                &state,
                &review.snapshot,
                &review.verdict,
                &review.findings,
                &review.tasks,
                review.architecture.as_ref(),
                required_architecture,
            )?;
            if review.assessment.len() > 16
                || review
                    .assessment
                    .iter()
                    .any(|entry| !text_valid(entry, 4096))
            {
                return Err("Review assessment requires bounded nonempty evidence entries".into());
            }
            if let Some(report) = architecture {
                state.last_architecture = Some(report);
            }
            if review.verdict == "changes" {
                repair(&mut state, review.snapshot, findings, &tasks, None)?;
            } else {
                state.phase = "qa".into();
                state.review_snapshot = Some(review.snapshot);
                state.findings = findings;
            }
        }
        "verify" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Verification {
                snapshot: String,
                proofs: Vec<String>,
            }
            let verification: Verification = report(tail)?;
            validate_proofs(
                project,
                &state,
                &verification.snapshot,
                &verification.proofs,
            )?;
            state.proofs = verification.proofs;
            for progress in state.task_states.values_mut() {
                progress.status = "verified".into();
            }
        }
        "qa" => {
            if state.phase != "qa" {
                return Err("QA requires accepted review".into());
            }
            let qa: Qa = report(tail)?;
            fresh(project, &state, &qa.snapshot)?;
            let plan = state.plan.as_ref().ok_or("QA requires a plan")?;
            let mut verification_findings = qa.findings.clone();
            let mut verification_tasks = qa.tasks.clone();
            let mut stored_verification = None;
            let mut verification_gaps = Vec::new();
            if !plan.verification.is_empty() || qa.verification.is_some() {
                validate_plan(project, plan, &state.required_checks)?;
                let results = qa
                    .verification
                    .as_ref()
                    .ok_or("QA requires an explicit result for every verification obligation")?;
                let source = project.snapshot()?;
                if source.hash != qa.snapshot {
                    return Err(
                        "Verification report must describe the actual current source snapshot"
                            .into(),
                    );
                }
                let feedback = verification::assess(
                    project,
                    &source,
                    &plan.verification,
                    &verification_scopes(plan),
                    &state.required_checks,
                    results,
                    &qa.proofs,
                    &qa.verdict,
                )?;
                // Empty legacy targets accompanying legacy findings still mean
                // all tasks. Obligation-only gaps have precise mandatory owners.
                if qa.findings.is_empty() || !qa.tasks.is_empty() {
                    for id in feedback.tasks {
                        if !verification_tasks.contains(&id) {
                            verification_tasks.push(id);
                        }
                    }
                }
                verification_findings.extend(feedback.findings);
                verification_gaps = feedback
                    .results
                    .iter()
                    .filter(|result| result.status == "gap")
                    .map(|result| result.id.clone())
                    .collect();
                verification_gaps.sort();
                if !plan.verification.is_empty() {
                    stored_verification = Some(verification::StoredReport {
                        snapshot: qa.snapshot.clone(),
                        contract_hash: verification_contract_hash(&state)?,
                        results: feedback.results,
                    });
                }
            }
            let required_architecture = state.mode == "single"
                && state
                    .plan
                    .as_ref()
                    .is_some_and(|plan| plan.architecture.is_some());
            let (findings, tasks, architecture) = architecture_feedback(
                project,
                &state,
                &qa.snapshot,
                &qa.verdict,
                &verification_findings,
                &verification_tasks,
                qa.architecture.as_ref(),
                required_architecture,
            )?;
            if qa.proofs.len() > 64 {
                return Err("Too many QA proofs".into());
            }
            if qa.verdict == "changes" {
                let config = project.config()?;
                let config_hash = sha(fs::read(project.state.join("config.json"))?);
                let mut diagnostics = BTreeMap::new();
                let mut ids = BTreeSet::new();
                let mut failed = Vec::new();
                for id in &qa.proofs {
                    if !ids.insert(id) {
                        return Err("Duplicate diagnostic proof".into());
                    }
                    let proof = verify::load(project, id)?;
                    let check = config
                        .checks
                        .get(&proof.check)
                        .ok_or("Diagnostic proof refers to unconfigured check")?;
                    if !state.required_checks.contains(&proof.check)
                        || proof.snapshot != qa.snapshot
                        || proof.snapshot_after.as_deref() != Some(&qa.snapshot)
                        || proof.config_hash != config_hash
                        || proof.argv != check.argv
                        || !proof.source_unchanged
                    {
                        return Err(
                            "Repair diagnostic proof must match this source/configuration".into(),
                        );
                    }
                    if !proof.passed {
                        diagnostics.insert(proof.check.clone(), value!({"exit":proof.exit_code,"timeout":proof.timed_out,"output_limited":proof.output_limited,
                            "stdout":sha(proof.stdout.as_bytes()),"stderr":sha(proof.stderr.as_bytes()),"execution_error":proof.execution_error}));
                        failed.push(id.clone());
                    }
                }
                let identity = if !verification_gaps.is_empty() {
                    // Rephrasing an unmet obligation is not source progress.
                    // Check output can contain durations, temporary paths or
                    // counters. Its full proof remains available to executors,
                    // but only check names and stable status belong in this key.
                    let statuses = diagnostics
                        .iter()
                        .map(|(name, diagnostic)| {
                            (
                                name,
                                &diagnostic["exit"],
                                &diagnostic["timeout"],
                                &diagnostic["output_limited"],
                                !diagnostic["execution_error"].is_null(),
                            )
                        })
                        .collect::<Vec<_>>();
                    Some(sha(serde_json::to_vec(&(
                        "verification-gaps",
                        verification_contract_hash(&state)?,
                        &verification_gaps,
                        &statuses,
                    ))?))
                } else if diagnostics.is_empty() {
                    None
                } else {
                    Some(sha(serde_json::to_vec(&diagnostics)?))
                };
                state.failed_proofs = failed;
                if let Some(report) = stored_verification {
                    state.last_verification = Some(report);
                }
                if state.mode == "single" {
                    if let Some(report) = architecture {
                        state.last_architecture = Some(report);
                    }
                }
                repair(&mut state, qa.snapshot, findings, &tasks, identity)?;
            } else {
                if state.review_snapshot.as_deref() != Some(&qa.snapshot) {
                    return Err("QA requires review of this exact snapshot".into());
                }
                validate_plan(
                    project,
                    state.plan.as_ref().ok_or("Missing plan")?,
                    &state.required_checks,
                )?;
                validate_proofs(project, &state, &qa.snapshot, &qa.proofs)?;
                if state.mode == "single"
                    && (qa.assessment.is_empty()
                        || qa.assessment.len() > 64
                        || qa.assessment.iter().any(|entry| !text_valid(entry, 4096)))
                {
                    return Err("Single-executor QA requires an explicit assessment of the acceptance criteria".into());
                }
                // A team QA acceptance must not replace its independent review
                // evidence. Its optional report remains in the host artifacts.
                if state.mode == "single" {
                    if let Some(report) = architecture {
                        state.last_architecture = Some(report);
                    }
                }
                state.proofs = qa.proofs;
                if let Some(report) = stored_verification {
                    state.last_verification = Some(report);
                }
                state.assessment = qa.assessment;
                state.failed_proofs.clear();
                state.findings = findings;
                for progress in state.task_states.values_mut() {
                    progress.status = "verified".into();
                }
                save(project, &state)?;
                for (id, progress) in &mut state.task_states {
                    progress.status = "completed".into();
                    state.completed.insert(id.clone(), state.snapshot.clone());
                }
                state.phase = "done".into();
            }
        }
        _ => return Err("Unknown workflow action or invalid options".into()),
    }
    save(project, &state)?;
    status(project, &state)
}
