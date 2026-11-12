//! Persistent task claims. The CLI holds the project lock across each operation.
use crate::{
    json,
    project::{normalize_path, scope_contains, sha, valid_id, Project, Snapshot},
    verify, Result,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

const STATE: &str = "tasks.json";
const MAX_TASKS: usize = 2_000;
const MAX_SCOPES: usize = 64;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum TaskStatus {
    Pending,
    Running,
    Done,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Task {
    id: String,
    title: String,
    paths: Vec<String>,
    depends: Vec<String>,
    status: TaskStatus,
    owner: Option<String>,
    proof: Option<String>,
    scope_hash: Option<String>,
    dependency_hash: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tasks {
    #[serde(default)]
    tasks: BTreeMap<String, Task>,
}

fn require_id(value: &str, label: &str) -> Result<()> {
    if !valid_id(value) {
        return Err(format!("Invalid {label}: use 1–80 ASCII letters, digits, '-' or '_'").into());
    }
    Ok(())
}

fn title_valid(title: &str) -> bool {
    !title.trim().is_empty() && title.len() <= 1_000 && !title.chars().any(char::is_control)
}

fn scope(value: &str) -> Result<String> {
    if value == "." {
        return Ok(value.into());
    }
    if value.contains(['*', '?', '[', ']']) {
        return Err("Task paths are exact files or directory prefixes, not globs".into());
    }
    normalize_path(value)
}

fn overlap(left: &str, right: &str) -> bool {
    scope_contains(left, right) || scope_contains(right, left)
}

fn load(project: &Project) -> Result<Tasks> {
    let tasks: Tasks = project.read_state(STATE)?;
    validate(&tasks)?;
    Ok(tasks)
}

fn save(project: &Project, tasks: &Tasks) -> Result<()> {
    validate(tasks)?;
    // Never write a task store that Project::read_state cannot read back.
    if serde_json::to_vec_pretty(tasks)?.len() >= 8 * 1024 * 1024 {
        return Err("Task state exceeds 8 MiB; use a smaller task set".into());
    }
    project.write_state(STATE, tasks)
}

fn validate(tasks: &Tasks) -> Result<()> {
    if tasks.tasks.len() > MAX_TASKS {
        return Err("Task limit is 2000".into());
    }
    for (key, task) in &tasks.tasks {
        require_id(key, "task ID")?;
        if key != &task.id
            || !title_valid(&task.title)
            || task.paths.is_empty()
            || task.paths.len() > MAX_SCOPES
            || task.depends.len() > MAX_SCOPES
        {
            return Err(format!("Invalid stored task: {key}").into());
        }
        for path in &task.paths {
            if scope(path)? != *path {
                return Err(format!("Noncanonical task scope: {path}").into());
            }
        }
        for dep in &task.depends {
            require_id(dep, "dependency ID")?;
            if dep == key || !tasks.tasks.contains_key(dep) {
                return Err(format!("Missing or self dependency {dep} for task {key}").into());
            }
        }
        match task.status {
            TaskStatus::Pending
                if task.owner.is_none()
                    && task.proof.is_none()
                    && task.scope_hash.is_none()
                    && task.dependency_hash.is_none() =>
            {
                ()
            }
            TaskStatus::Running
                if task.owner.is_some()
                    && task.proof.is_none()
                    && task.scope_hash.is_none()
                    && task.dependency_hash.is_none() =>
            {
                ()
            }
            TaskStatus::Done
                if task.owner.is_some()
                    && task.proof.is_some()
                    && task.scope_hash.as_ref().is_some_and(|h| {
                        h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())
                    })
                    && task.dependency_hash.as_ref().is_some_and(|h| {
                        h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())
                    }) =>
            {
                ()
            }
            _ => return Err(format!("Inconsistent stored task status: {key}").into()),
        }
        if let Some(owner) = &task.owner {
            require_id(owner, "owner")?;
        }
        if let Some(proof) = &task.proof {
            require_id(proof, "proof ID")?;
        }
    }
    // Iterative DFS avoids recursion proportional to a user-created task graph.
    let mut colors: BTreeMap<&str, u8> = BTreeMap::new();
    for id in tasks.tasks.keys() {
        let mut stack = vec![(id.as_str(), false)];
        while let Some((id, exiting)) = stack.pop() {
            if exiting {
                colors.insert(id, 2);
                continue;
            }
            match colors.get(id).copied().unwrap_or(0) {
                2 => continue,
                1 => return Err(format!("Task dependency cycle includes {id}").into()),
                _ => (),
            }
            colors.insert(id, 1);
            stack.push((id, true));
            for dep in tasks.tasks[id].depends.iter().rev() {
                stack.push((dep, false));
            }
        }
    }
    Ok(())
}

struct DependencyState {
    scopes: BTreeMap<String, String>,
    signatures: BTreeMap<String, String>,
}

impl DependencyState {
    fn new(tasks: &Tasks, snapshot: &Snapshot) -> Self {
        let scopes: BTreeMap<_, _> = tasks
            .tasks
            .values()
            .map(|task| (task.id.clone(), snapshot.scope_hash(&task.paths)))
            .collect();
        let mut signatures = BTreeMap::<String, String>::new();
        // The stored graph is already validated. Merkle fingerprints include all
        // transitive scoped inputs without expanding a diamond graph repeatedly.
        for id in tasks.tasks.keys() {
            let mut stack = vec![(id.as_str(), false)];
            while let Some((id, exiting)) = stack.pop() {
                if signatures.contains_key(id) {
                    continue;
                }
                if !exiting {
                    stack.push((id, true));
                    for dep in tasks.tasks[id].depends.iter().rev() {
                        if !signatures.contains_key(dep) {
                            stack.push((dep, false));
                        }
                    }
                    continue;
                }
                let mut data = b"ai-env-dependencies-1\0".to_vec();
                let mut dependencies = tasks.tasks[id].depends.clone();
                dependencies.sort();
                dependencies.dedup();
                for dep in dependencies {
                    for part in [&dep, &scopes[&dep], &signatures[&dep]] {
                        data.extend_from_slice(part.as_bytes());
                        data.push(0);
                    }
                }
                signatures.insert(id.to_owned(), sha(data));
            }
        }
        Self { scopes, signatures }
    }

    fn matches_completion(&self, task: &Task) -> bool {
        task.scope_hash.as_deref() == Some(self.scopes[&task.id].as_str())
            && task.dependency_hash.as_deref() == Some(self.signatures[&task.id].as_str())
    }
}

fn dependencies_fresh(tasks: &Tasks, task: &Task, current: &DependencyState) -> Result<()> {
    let mut remaining: Vec<&str> = task.depends.iter().map(String::as_str).collect();
    let mut seen = BTreeSet::new();
    while let Some(id) = remaining.pop() {
        if !seen.insert(id) {
            continue;
        }
        let dependency = tasks
            .tasks
            .get(id)
            .ok_or_else(|| format!("Missing dependency: {id}"))?;
        if dependency.status != TaskStatus::Done {
            return Err(format!("Dependency {id} is not done").into());
        }
        if !current.matches_completion(dependency) {
            return Err(format!(
                "Dependency {id} is stale: its scoped files or dependency inputs changed; claim and verify it again"
            )
            .into());
        }
        remaining.extend(dependency.depends.iter().map(String::as_str));
    }
    Ok(())
}

fn owned<'a>(tasks: &'a Tasks, id: &str, owner: &str) -> Result<&'a Task> {
    require_id(id, "task ID")?;
    require_id(owner, "owner")?;
    let task = tasks
        .tasks
        .get(id)
        .ok_or_else(|| format!("Unknown task: {id}"))?;
    if task.status != TaskStatus::Running || task.owner.as_deref() != Some(owner) {
        return Err(format!("Task {id} is not running under owner {owner}").into());
    }
    Ok(task)
}

fn check_paths(task: &Task, paths: &[String]) -> Result<()> {
    for path in paths {
        let normalized = normalize_path(path)?;
        if normalized != *path || !task.paths.iter().any(|scope| scope_contains(scope, path)) {
            return Err(format!("Path {path} is outside task {} scopes", task.id).into());
        }
    }
    Ok(())
}

/// Require an active claim, correct owner, current dependencies and in-scope edits.
/// The caller holds the same project lock used for the supplied fresh snapshot.
pub fn authorize(
    project: &Project,
    snapshot: &Snapshot,
    id: &str,
    owner: &str,
    paths: &[String],
) -> Result<()> {
    let tasks = load(project)?;
    let task = owned(&tasks, id, owner)?;
    dependencies_fresh(&tasks, task, &DependencyState::new(&tasks, snapshot))?;
    check_paths(task, paths)
}

/// An unscoped edit must also respect other agents' active claims.
pub fn check_claims(
    project: &Project,
    task: Option<&str>,
    owner: Option<&str>,
    paths: &[String],
) -> Result<()> {
    if task.is_some() != owner.is_some() {
        return Err("--task and --owner must be provided together".into());
    }
    let tasks = load(project)?;
    if let (Some(id), Some(owner)) = (task, owner) {
        check_paths(owned(&tasks, id, owner)?, paths)?;
    }
    for path in paths {
        if normalize_path(path)? != *path {
            return Err("Edit paths must be canonical".into());
        }
        for active in tasks
            .tasks
            .values()
            .filter(|entry| entry.status == TaskStatus::Running)
        {
            if Some(active.id.as_str()) == task {
                continue;
            }
            if active.paths.iter().any(|scope| overlap(scope, path)) {
                return Err(format!(
                    "Path {path} is claimed by running task {} (owner {})",
                    active.id,
                    active.owner.as_deref().unwrap_or("unknown")
                )
                .into());
            }
        }
    }
    Ok(())
}

fn successful_current_proof(
    project: &Project,
    snapshot: &Snapshot,
    proof: &verify::Proof,
) -> Result<bool> {
    let config = project.config()?;
    let configured = config
        .checks
        .get(&proof.check)
        .is_some_and(|check| check.argv == proof.argv);
    let config_hash = sha(fs::read(project.state.join("config.json"))?);
    Ok(proof.passed
        && proof.exit_code == Some(0)
        && !proof.timed_out
        && !proof.output_limited
        && proof.source_unchanged
        && proof.capture_complete
        && proof.execution_error.is_none()
        && proof.snapshot_after.as_deref() == Some(snapshot.hash.as_str())
        && proof.config_hash == config_hash
        && proof.snapshot == snapshot.hash
        && configured)
}

fn task_view(tasks: &Tasks, task: &Task, current: &DependencyState) -> serde_json::Value {
    let scope_fresh = task
        .scope_hash
        .as_deref()
        .map(|hash| hash == &current.scopes[&task.id]);
    let dependency_inputs_fresh = task
        .dependency_hash
        .as_deref()
        .map(|hash| hash == current.signatures[&task.id]);
    let dependency_error = dependencies_fresh(tasks, task, current)
        .err()
        .map(|e| e.to_string());
    let effective = match task.status {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::Done
            if scope_fresh == Some(true)
                && dependency_inputs_fresh == Some(true)
                && dependency_error.is_none() =>
        {
            "done"
        }
        TaskStatus::Done => "stale",
    };
    serde_json::json!({"id":task.id,"title":task.title,"paths":task.paths,"depends":task.depends,
        "status":effective,"owner":task.owner,"proof":task.proof,"scope_hash":task.scope_hash,
        "scope_fresh":scope_fresh,"dependency_hash":task.dependency_hash,"dependency_inputs_fresh":dependency_inputs_fresh,
        "dependencies_fresh":dependency_error.is_none(),"dependency_error":dependency_error})
}

fn fresh_proofs(project: &Project, snapshot: &Snapshot) -> Result<Vec<serde_json::Value>> {
    let directory = project.state.join("proofs");
    let metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("Invalid proof directory".into());
    }
    let mut ids = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(id) = name.strip_suffix(".json") {
            require_id(id, "proof ID")?;
            ids.push((entry.metadata()?.modified()?, id.to_owned()));
        }
    }
    ids.sort();
    let mut current = BTreeMap::new();
    for (_, id) in ids {
        let proof = verify::load(project, &id)?;
        if successful_current_proof(project, snapshot, &proof)? {
            // Repeated checks at one snapshot must not inflate handoff context.
            current.insert(proof.check.clone(), serde_json::json!({"id":proof.id,"check":proof.check,"snapshot":proof.snapshot,"passed":true}));
        }
    }
    Ok(current.into_values().collect())
}

/// Compact project status includes only currently successful proof summaries.
pub fn status(project: &Project, args: &[String]) -> Result<String> {
    if !args.is_empty() {
        return Err("Usage: aienv status".into());
    }
    let snapshot = project.snapshot()?;
    let tasks = load(project)?;
    let current = DependencyState::new(&tasks, &snapshot);
    let views: Vec<_> = tasks
        .tasks
        .values()
        .map(|task| task_view(&tasks, task, &current))
        .collect();
    json(
        &serde_json::json!({"snapshot":snapshot.hash,"omitted":snapshot.omitted,"tasks":views,"fresh_proofs":fresh_proofs(project,&snapshot)?}),
    )
}

fn options<'a>(args: &'a [String], allowed: &[&str]) -> Result<BTreeMap<&'a str, &'a str>> {
    let mut values = BTreeMap::new();
    let mut cursor = 0;
    while cursor < args.len() {
        let flag = args[cursor].as_str();
        if !allowed.contains(&flag) {
            return Err(format!("Unknown task option: {flag}").into());
        }
        let value = args
            .get(cursor + 1)
            .ok_or_else(|| format!("Missing value for {flag}"))?;
        if values.insert(flag, value.as_str()).is_some() {
            return Err(format!("Duplicate task option: {flag}").into());
        }
        cursor += 2;
    }
    Ok(values)
}

fn required<'a>(values: &BTreeMap<&str, &'a str>, key: &str) -> Result<&'a str> {
    values
        .get(key)
        .copied()
        .ok_or_else(|| format!("Missing task option {key}").into())
}

fn comma_list(value: &str, kind: &str) -> Result<Vec<String>> {
    let mut values = Vec::new();
    for part in value.split(',') {
        if part.is_empty() {
            return Err(format!("Empty {kind} in comma-separated list").into());
        }
        values.push(part.to_owned());
    }
    values.sort();
    values.dedup();
    if values.len() > MAX_SCOPES {
        return Err(format!("Maximum {MAX_SCOPES} {kind}s per task").into());
    }
    Ok(values)
}

pub fn run(project: &Project, args: &[String]) -> Result<String> {
    let action = args
        .first()
        .map(String::as_str)
        .ok_or("Usage: aienv task add|claim|complete|release|list|show")?;
    if action == "list" {
        if args.len() != 1 {
            return Err("Usage: aienv task list".into());
        }
        return status(project, &[]);
    }
    if !["add", "claim", "complete", "release", "show"].contains(&action) {
        return Err(format!("Unknown task action: {action}").into());
    }
    let id = args.get(1).ok_or("Expected a task ID")?;
    require_id(id, "task ID")?;
    let mut tasks = load(project)?;
    let snapshot = project.snapshot()?;
    let current = DependencyState::new(&tasks, &snapshot);
    match action {
        "add" => {
            let values = options(&args[2..], &["--title", "--paths", "--depends"])?;
            let title = required(&values, "--title")?;
            if !title_valid(title) {
                return Err("Task title must be nonempty, at most 1000 bytes and contain no control characters".into());
            }
            if tasks.tasks.contains_key(id) {
                return Err(format!("Task already exists: {id}").into());
            }
            let paths = comma_list(required(&values, "--paths")?, "scope")?
                .into_iter()
                .map(|value| scope(&value))
                .collect::<Result<Vec<_>>>()?;
            let depends = values
                .get("--depends")
                .map(|value| comma_list(value, "dependency"))
                .transpose()?
                .unwrap_or_default();
            for dep in &depends {
                require_id(dep, "dependency ID")?;
                if dep == id {
                    return Err("A task cannot depend on itself".into());
                }
                if !tasks.tasks.contains_key(dep) {
                    return Err(format!("Unknown dependency: {dep}; add it first").into());
                }
            }
            tasks.tasks.insert(
                id.clone(),
                Task {
                    id: id.clone(),
                    title: title.into(),
                    paths,
                    depends,
                    status: TaskStatus::Pending,
                    owner: None,
                    proof: None,
                    scope_hash: None,
                    dependency_hash: None,
                },
            );
        }
        "show" => {
            if args.len() != 2 {
                return Err("Usage: aienv task show ID".into());
            }
            let task = tasks
                .tasks
                .get(id)
                .ok_or_else(|| format!("Unknown task: {id}"))?;
            return json(
                &serde_json::json!({"snapshot":snapshot.hash,"task":task_view(&tasks,task,&current)}),
            );
        }
        "claim" => {
            let values = options(&args[2..], &["--owner"])?;
            let owner = required(&values, "--owner")?;
            require_id(owner, "owner")?;
            let task = tasks
                .tasks
                .get(id)
                .ok_or_else(|| format!("Unknown task: {id}"))?;
            if task.status == TaskStatus::Running {
                return Err(format!("Task {id} is already claimed").into());
            }
            if task.status == TaskStatus::Done
                && current.matches_completion(task)
                && dependencies_fresh(&tasks, task, &current).is_ok()
            {
                return Err(format!("Task {id} is already done and its scope is fresh").into());
            }
            dependencies_fresh(&tasks, task, &current)?;
            for active in tasks
                .tasks
                .values()
                .filter(|entry| entry.status == TaskStatus::Running)
            {
                if task
                    .paths
                    .iter()
                    .any(|left| active.paths.iter().any(|right| overlap(left, right)))
                {
                    return Err(format!(
                        "Task {id} overlaps running task {} (owner {})",
                        active.id,
                        active.owner.as_deref().unwrap_or("unknown")
                    )
                    .into());
                }
            }
            let task = tasks.tasks.get_mut(id).unwrap();
            task.status = TaskStatus::Running;
            task.owner = Some(owner.into());
            task.proof = None;
            task.scope_hash = None;
            task.dependency_hash = None;
        }
        "complete" => {
            let values = options(&args[2..], &["--owner", "--proof"])?;
            let owner = required(&values, "--owner")?;
            let task = owned(&tasks, id, owner)?;
            dependencies_fresh(&tasks, task, &current)?;
            let proof_id = required(&values, "--proof")?;
            require_id(proof_id, "proof ID")?;
            let proof = verify::load(project, proof_id)?;
            if proof.id != proof_id || !successful_current_proof(project, &snapshot, &proof)? {
                return Err("Completion needs a successful configured-check proof for the current source/config snapshot; run aienv verify CHECK".into());
            }
            if project.snapshot()?.hash != snapshot.hash {
                return Err(
                    "Source/config changed during task completion; refresh and verify again".into(),
                );
            }
            let scope_hash = snapshot.scope_hash(&task.paths);
            let task = tasks.tasks.get_mut(id).unwrap();
            task.status = TaskStatus::Done;
            task.proof = Some(proof_id.into());
            task.scope_hash = Some(scope_hash);
            task.dependency_hash = Some(current.signatures[id].clone());
        }
        "release" => {
            let values = options(&args[2..], &["--owner"])?;
            let owner = required(&values, "--owner")?;
            owned(&tasks, id, owner)?;
            let task = tasks.tasks.get_mut(id).unwrap();
            task.status = TaskStatus::Pending;
            task.owner = None;
            task.proof = None;
            task.scope_hash = None;
            task.dependency_hash = None;
        }
        _ => unreachable!(),
    }
    save(project, &tasks)?;
    let current = DependencyState::new(&tasks, &snapshot);
    json(
        &serde_json::json!({"snapshot":snapshot.hash,"task":task_view(&tasks,&tasks.tasks[id],&current)}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    struct Fixture {
        project: Project,
        path: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ai-env-task-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::create_dir(path.join("src")).unwrap();
            fs::write(path.join("src/a.rs"), "fn a() {}\n").unwrap();
            fs::write(path.join("src/b.rs"), "fn b() {}\n").unwrap();
            let project = Project::init(&path).unwrap();
            project.write_state("config.json", &serde_json::json!({"checks":{"qa":{"argv":["/usr/bin/true"],"timeout_seconds":10}}})).unwrap();
            Self { project, path }
        }
        fn command(&self, args: &[&str]) -> Result<serde_json::Value> {
            let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            Ok(serde_json::from_str(&run(&self.project, &args)?)?)
        }
        fn add(&self, id: &str, paths: &str, depends: Option<&str>) {
            let mut args = vec!["add", id, "--title", id, "--paths", paths];
            if let Some(dep) = depends {
                args.extend(["--depends", dep]);
            }
            self.command(&args).unwrap();
        }
        fn proof(&self, id: &str, passed: bool) {
            let snapshot = self.project.snapshot().unwrap();
            self.project.write_state(&format!("proofs/{id}.json"), &serde_json::json!({
                "id":id,"snapshot":snapshot.hash,"check":"qa","argv":["/usr/bin/true"],"passed":passed,
                "exit_code":if passed {0}else{1},"stdout":"","stderr":"","timed_out":false,"output_limited":false,
                "config_hash":sha(fs::read(self.project.state.join("config.json")).unwrap()),
                "source_unchanged":true,"capture_complete":true,"snapshot_after":snapshot.hash,"execution_error":null
            })).unwrap();
        }
        fn finish(&self, id: &str) {
            self.command(&["claim", id, "--owner", "agent"]).unwrap();
            self.proof("proof", true);
            self.command(&["complete", id, "--owner", "agent", "--proof", "proof"])
                .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn claims_reject_parent_child_overlap_even_for_same_owner_and_release_unlocks() {
        let f = Fixture::new();
        f.add("parent", "src", None);
        f.add("child", "src/a.rs", None);
        f.add("sibling", "docs", None);
        f.command(&["claim", "parent", "--owner", "agent"]).unwrap();
        assert!(f.command(&["claim", "child", "--owner", "agent"]).is_err());
        assert!(f.command(&["claim", "child", "--owner", "other"]).is_err());
        f.command(&["claim", "sibling", "--owner", "other"])
            .unwrap();
        assert!(f
            .command(&["release", "parent", "--owner", "other"])
            .is_err());
        f.command(&["release", "parent", "--owner", "agent"])
            .unwrap();
        f.command(&["claim", "child", "--owner", "agent"]).unwrap();
    }

    #[test]
    fn whole_project_scope_conflicts_but_path_prefixes_are_component_boundaries() {
        let f = Fixture::new();
        f.add("a", "src/a.rs", None);
        f.add("similar", "src/a.rs-more", None);
        f.add("all", ".", None);
        f.command(&["claim", "a", "--owner", "one"]).unwrap();
        f.command(&["claim", "similar", "--owner", "two"]).unwrap();
        assert!(f.command(&["claim", "all", "--owner", "three"]).is_err());
    }

    #[test]
    fn completion_requires_current_successful_proof_and_owner_and_config() {
        let f = Fixture::new();
        f.add("a", "src/a.rs", None);
        f.command(&["claim", "a", "--owner", "agent"]).unwrap();
        assert!(f
            .command(&["complete", "a", "--owner", "other", "--proof", "none"])
            .is_err());
        assert!(f
            .command(&["complete", "a", "--owner", "agent", "--proof", "none"])
            .is_err());
        f.proof("bad", false);
        assert!(f
            .command(&["complete", "a", "--owner", "agent", "--proof", "bad"])
            .is_err());
        f.proof("old", true);
        fs::write(f.path.join("src/b.rs"), "fn changed() {}\n").unwrap();
        assert!(f
            .command(&["complete", "a", "--owner", "agent", "--proof", "old"])
            .is_err());
        f.proof("config", true);
        f.project.write_state("config.json",&serde_json::json!({"checks":{"qa":{"argv":["/usr/bin/false"],"timeout_seconds":10}}})).unwrap();
        assert!(f
            .command(&["complete", "a", "--owner", "agent", "--proof", "config"])
            .is_err());
    }

    #[test]
    fn a_passed_flag_cannot_override_inconsistent_check_evidence() {
        let f = Fixture::new();
        f.add("a", "src/a.rs", None);
        f.command(&["claim", "a", "--owner", "agent"]).unwrap();
        let contradictions = [
            ("exit_code", serde_json::json!(1)),
            ("timed_out", serde_json::json!(true)),
            ("output_limited", serde_json::json!(true)),
            ("source_unchanged", serde_json::json!(false)),
            ("capture_complete", serde_json::json!(false)),
            ("execution_error", serde_json::json!("capture failed")),
            ("snapshot_after", serde_json::json!(null)),
            ("snapshot_after", serde_json::json!("different")),
            ("config_hash", serde_json::json!("different")),
            ("config_hash", serde_json::json!("")),
            ("argv", serde_json::json!(["/usr/bin/false"])),
            ("check", serde_json::json!("unknown")),
            ("id", serde_json::json!("different")),
        ];
        for (field, value) in contradictions {
            f.proof("case", true);
            let mut proof: serde_json::Value = f.project.read_state("proofs/case.json").unwrap();
            proof[field] = value;
            f.project.write_state("proofs/case.json", &proof).unwrap();
            assert!(
                f.command(&["complete", "a", "--owner", "agent", "--proof", "case"])
                    .is_err(),
                "{field}"
            );
            assert_eq!(
                f.command(&["show", "a"]).unwrap()["task"]["status"],
                "running"
            );
        }
        f.proof("valid", true);
        let done = f
            .command(&["complete", "a", "--owner", "agent", "--proof", "valid"])
            .unwrap();
        assert_eq!(done["task"]["status"], "done");
    }

    #[test]
    fn done_scope_stays_fresh_after_unrelated_change_but_stales_on_own_change() {
        let f = Fixture::new();
        f.add("a", "src/a.rs", None);
        f.finish("a");
        fs::write(f.path.join("src/b.rs"), "fn unrelated() {}\n").unwrap();
        let show = f.command(&["show", "a"]).unwrap();
        assert_eq!(show["task"]["status"], "done");
        assert!(f.command(&["claim", "a", "--owner", "other"]).is_err());
        fs::write(f.path.join("src/a.rs"), "fn changed() {}\n").unwrap();
        let show = f.command(&["show", "a"]).unwrap();
        assert_eq!(show["task"]["status"], "stale");
        f.command(&["claim", "a", "--owner", "other"]).unwrap();
    }

    #[test]
    fn dependencies_gate_claim_and_authorization_and_detect_transitive_staleness() {
        let f = Fixture::new();
        f.add("a", "src/a.rs", None);
        f.add("b", "src/b.rs", Some("a"));
        f.add("c", "docs", Some("b"));
        assert!(f.command(&["claim", "b", "--owner", "agent"]).is_err());
        f.finish("a");
        f.finish("b");
        f.command(&["claim", "c", "--owner", "other"]).unwrap();
        let snapshot = f.project.snapshot().unwrap();
        assert!(authorize(
            &f.project,
            &snapshot,
            "c",
            "other",
            &["docs/readme.md".into()]
        )
        .is_ok());
        fs::write(f.path.join("src/a.rs"), "fn changed() {}\n").unwrap();
        let snapshot = f.project.snapshot().unwrap();
        assert!(authorize(
            &f.project,
            &snapshot,
            "c",
            "other",
            &["docs/readme.md".into()]
        )
        .is_err());
        assert!(f
            .command(&["complete", "c", "--owner", "other", "--proof", "proof"])
            .is_err());
    }

    #[test]
    fn completed_downstream_task_stays_stale_after_dependency_reverification() {
        let f = Fixture::new();
        f.add("upstream", "src/a.rs", None);
        f.add("downstream", "src/b.rs", Some("upstream"));
        f.finish("upstream");
        f.finish("downstream");
        let original = f.command(&["show", "downstream"]).unwrap();
        let original_dependencies = original["task"]["dependency_hash"].clone();
        fs::write(f.path.join("unrelated.md"), "an unrelated change").unwrap();
        assert_eq!(
            f.command(&["show", "downstream"]).unwrap()["task"]["status"],
            "done"
        );

        fs::write(f.path.join("src/a.rs"), "fn upstream_changed() {}\n").unwrap();
        let stale = f.command(&["show", "downstream"]).unwrap();
        assert_eq!(stale["task"]["status"], "stale");
        assert_eq!(stale["task"]["scope_fresh"], true);
        assert_eq!(stale["task"]["dependencies_fresh"], false);
        assert!(f
            .command(&["claim", "downstream", "--owner", "new-agent"])
            .is_err());

        f.finish("upstream");
        let stale = f.command(&["show", "downstream"]).unwrap();
        assert_eq!(stale["task"]["status"], "stale");
        assert_eq!(stale["task"]["dependencies_fresh"], true);
        assert_eq!(stale["task"]["dependency_inputs_fresh"], false);
        f.command(&["claim", "downstream", "--owner", "new-agent"])
            .unwrap();
        f.proof("downstream-proof", true);
        let completed = f
            .command(&[
                "complete",
                "downstream",
                "--owner",
                "new-agent",
                "--proof",
                "downstream-proof",
            ])
            .unwrap();
        assert_eq!(completed["task"]["status"], "done");
        assert_ne!(completed["task"]["dependency_hash"], original_dependencies);
    }

    #[test]
    fn edits_need_owner_scope_and_cannot_bypass_another_running_claim() {
        let f = Fixture::new();
        f.add("a", "src/a.rs", None);
        f.command(&["claim", "a", "--owner", "agent"]).unwrap();
        let path = vec!["src/a.rs".into()];
        let snapshot = f.project.snapshot().unwrap();
        assert!(check_claims(&f.project, None, None, &path).is_err());
        assert!(check_claims(&f.project, Some("a"), Some("other"), &path).is_err());
        assert!(check_claims(&f.project, Some("a"), None, &path).is_err());
        check_claims(&f.project, Some("a"), Some("agent"), &path).unwrap();
        authorize(&f.project, &snapshot, "a", "agent", &path).unwrap();
        assert!(authorize(&f.project, &snapshot, "a", "agent", &["src/b.rs".into()]).is_err());
        assert!(check_claims(&f.project, None, None, &["src".into()]).is_err());
        check_claims(&f.project, None, None, &["src/b.rs".into()]).unwrap();
    }

    #[test]
    fn dependencies_and_scope_syntax_fail_closed_without_mutating_state() {
        let f = Fixture::new();
        f.add("a", "src/a.rs", None);
        assert!(f
            .command(&[
                "add",
                "b",
                "--title",
                "B",
                "--paths",
                "docs",
                "--depends",
                "missing"
            ])
            .is_err());
        assert!(f
            .command(&[
                "add",
                "b",
                "--title",
                "B",
                "--paths",
                "docs",
                "--depends",
                "b"
            ])
            .is_err());
        for bad in [
            "../outside",
            "/tmp/outside",
            ".git/config",
            ".ai-env/tasks.json",
            "src/*.rs",
        ] {
            assert!(
                f.command(&["add", "bad", "--title", "B", "--paths", bad])
                    .is_err(),
                "{bad}"
            );
        }
        assert_eq!(load(&f.project).unwrap().tasks.len(), 1);
        let mut state = load(&f.project).unwrap();
        let mut b = state.tasks["a"].clone();
        b.id = "b".into();
        b.depends = vec!["a".into()];
        state.tasks.insert("b".into(), b);
        state.tasks.get_mut("a").unwrap().depends = vec!["b".into()];
        f.project.write_state(STATE, &state).unwrap();
        assert!(status(&f.project, &[]).is_err());
    }

    #[test]
    fn status_lists_only_fresh_successful_proofs_and_metadata_does_not_stale_them() {
        let f = Fixture::new();
        f.proof("good", true);
        f.proof("bad", false);
        f.add("a", "src/a.rs", None);
        let status: serde_json::Value =
            serde_json::from_str(&super::status(&f.project, &[]).unwrap()).unwrap();
        assert_eq!(status["fresh_proofs"].as_array().unwrap().len(), 1);
        assert_eq!(status["fresh_proofs"][0]["id"], "good");
        f.proof("another-good", true);
        let status: serde_json::Value =
            serde_json::from_str(&super::status(&f.project, &[]).unwrap()).unwrap();
        assert_eq!(status["fresh_proofs"].as_array().unwrap().len(), 1);
        f.command(&["claim", "a", "--owner", "agent"]).unwrap();
        let status: serde_json::Value =
            serde_json::from_str(&super::status(&f.project, &[]).unwrap()).unwrap();
        assert_eq!(status["fresh_proofs"].as_array().unwrap().len(), 1);
        fs::write(f.path.join("src/a.rs"), "fn changed() {}\n").unwrap();
        let status: serde_json::Value =
            serde_json::from_str(&super::status(&f.project, &[]).unwrap()).unwrap();
        assert!(status["fresh_proofs"].as_array().unwrap().is_empty());
    }
}
