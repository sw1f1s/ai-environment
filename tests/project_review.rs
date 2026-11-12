//! Independent project/decision regressions; no network or model access.
use aienv::{notes, project::Project};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "aienv-project-review-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn project(&self) -> Project {
        Project::init(&self.0).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}

fn git_repo(path: &Path) -> bool {
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .arg(path)
        .status();
    match status {
        Ok(status) => {
            assert!(status.success());
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => panic!("git init failed: {error}"),
    }
}

#[test]
fn nested_git_project_respects_parent_ignore_rules_in_its_index() {
    let fixture = Fixture::new();
    if !git_repo(&fixture.0) {
        return;
    }
    fs::write(
        fixture.0.join(".gitignore"),
        "component/private.txt\ncomponent/ignored-cache/\n",
    )
    .unwrap();
    let component = fixture.0.join("component");
    fs::create_dir(&component).unwrap();
    fs::create_dir(component.join("ignored-cache")).unwrap();
    fs::write(component.join("visible.rs"), "pub fn public() {}\n").unwrap();
    fs::write(component.join("private.txt"), "PRIVATE CONTENT\n").unwrap();
    fs::write(component.join("ignored-cache/cache.txt"), "CACHE CONTENT\n").unwrap();
    fs::write(fixture.0.join("outside.rs"), "OUTSIDE PROJECT\n").unwrap();
    let project = Project::init(&component).unwrap();
    let _lock = project.lock().unwrap();
    let snapshot = project.snapshot().unwrap();
    assert!(snapshot.files.contains_key("visible.rs"));
    assert!(
        !snapshot.files.contains_key("private.txt"),
        "Parent .gitignore must apply to a nested project root"
    );
    assert!(!snapshot.files.contains_key("ignored-cache/cache.txt"));
    assert!(!snapshot.files.contains_key("outside.rs"));
}

#[test]
fn nested_git_project_rejects_creating_ignored_source() {
    let fixture = Fixture::new();
    if !git_repo(&fixture.0) {
        return;
    }
    fs::write(fixture.0.join(".gitignore"), "component/private.txt\n").unwrap();
    let component = fixture.0.join("component");
    fs::create_dir(&component).unwrap();
    let project = Project::init(&component).unwrap();
    let _lock = project.lock().unwrap();
    assert!(project.allows_new("visible.rs").is_ok());
    assert!(project.allows_new("private.txt").is_err());
}

#[cfg(unix)]
#[test]
fn literal_backslash_filename_is_explicitly_omitted_not_reinterpreted_as_a_path() {
    let fixture = Fixture::new();
    let project = fixture.project();
    let _lock = project.lock().unwrap();
    let before = project.snapshot().unwrap();
    fs::write(fixture.0.join("literal\\name.rs"), "not a directory\n").unwrap();
    let after = project.snapshot().unwrap();
    assert_eq!(after.files.len(), before.files.len());
    assert_eq!(after.omitted, before.omitted + 1);
    assert_eq!(after.hash, before.hash);
}

#[test]
fn notes_and_task_metadata_do_not_change_source_snapshot_but_config_does() {
    let fixture = Fixture::new();
    fs::write(fixture.0.join("app.rs"), "fn main() {}\n").unwrap();
    let project = fixture.project();
    let _lock = project.lock().unwrap();
    let before = project.snapshot().unwrap();
    notes::run(
        &project,
        &args(&[
            "set",
            "decision",
            "--text",
            "Use local state",
            "--paths",
            "app.rs",
        ]),
    )
    .unwrap();
    project
        .write_state(
            "arbitrary-metadata.json",
            &serde_json::json!({"state": "changed"}),
        )
        .unwrap();
    assert_eq!(project.snapshot().unwrap().hash, before.hash);
    fs::write(project.state.join("config.json"), "{\n\"checks\":{}\n}\n").unwrap();
    assert_ne!(project.snapshot().unwrap().hash, before.hash);
}

#[test]
fn decisions_become_stale_only_when_their_scoped_sources_change() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("package")).unwrap();
    fs::write(
        fixture.0.join("package/lib.rs"),
        "pub fn value() -> u32 { 1 }\n",
    )
    .unwrap();
    fs::write(fixture.0.join("unrelated.txt"), "first\n").unwrap();
    let project = fixture.project();
    let _lock = project.lock().unwrap();
    let list = notes::run(
        &project,
        &args(&[
            "set",
            "state",
            "--text",
            "Keep state local",
            "--paths",
            "package",
        ]),
    )
    .unwrap();
    let saved: Value = serde_json::from_str(&list).unwrap();
    assert_eq!(saved["saved"], true);
    let parsed: Value = serde_json::from_str(&notes::list(&project).unwrap()).unwrap();
    assert_eq!(parsed["notes"][0]["stale"], false);
    fs::write(fixture.0.join("unrelated.txt"), "second\n").unwrap();
    let parsed: Value = serde_json::from_str(&notes::list(&project).unwrap()).unwrap();
    assert_eq!(parsed["notes"][0]["stale"], false);
    fs::write(fixture.0.join("package/new.rs"), "pub fn added() {}\n").unwrap();
    let parsed: Value = serde_json::from_str(&notes::list(&project).unwrap()).unwrap();
    assert_eq!(parsed["notes"][0]["stale"], true);
}

#[cfg(unix)]
#[test]
fn linked_source_and_state_paths_are_rejected_without_touching_targets() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let target = outside.0.join("target.txt");
    fs::write(&target, "unchanged\n").unwrap();
    let project = fixture.project();
    let _lock = project.lock().unwrap();
    symlink(&outside.0, fixture.0.join("linked-dir")).unwrap();
    symlink(&target, fixture.0.join("linked.rs")).unwrap();
    assert!(project.resolve("linked-dir/target.txt").is_err());
    assert!(project.resolve("linked.rs").is_err());
    assert!(!project.snapshot().unwrap().files.contains_key("linked.rs"));
    symlink(&target, project.state.join("notes.json")).unwrap();
    assert!(project.read_state::<Value>("notes.json").is_err());
    assert!(project
        .write_state("notes.json", &serde_json::json!({}))
        .is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "unchanged\n");
}

#[test]
fn installed_tools_and_generated_csharp_do_not_become_application_sources() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.0.join("src")).unwrap();
    fs::write(fixture.0.join("src/App.cs"), "class App {}\n").unwrap();
    for directory in [
        ".agents/skills/ai-env/analyzer",
        ".codex/tools",
        "obj/Debug",
        ".vs/cache",
    ] {
        let path = fixture.0.join(directory);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("Tool.cs"), "class Tool { Missing type; }\n").unwrap();
    }
    let project = fixture.project();
    let snapshot = project.snapshot().unwrap();
    assert_eq!(
        snapshot.files.keys().cloned().collect::<Vec<_>>(),
        ["src/App.cs"]
    );
    let initial = snapshot.hash;
    fs::write(
        fixture.0.join("obj/Debug/Tool.cs"),
        "class ChangedGeneratedTool {}\n",
    )
    .unwrap();
    assert_eq!(project.snapshot().unwrap().hash, initial);
}
