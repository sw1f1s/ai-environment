use aienv::project::{sha, Project};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
    base: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "ai-env-cli-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let root = base.join("repo");
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(
            root.join("inventory.py"),
            "def refund(stock, allocated):\n    return stock\n",
        )
        .unwrap();
        fs::write(root.join("orders.py"),"from inventory import refund\ndef cancel(stock, allocated):\n    return refund(stock, allocated)\n").unwrap();
        fs::write(root.join("tests/test_orders.py"),"import unittest\nfrom orders import cancel\nclass Orders(unittest.TestCase):\n    def test_partial(self): self.assertEqual(cancel(10, 3), 13)\n    def test_empty(self): self.assertEqual(cancel(0, 0), 0)\n").unwrap();
        let f = Self { root, base };
        assert!(f.run(&["init"]).status.success());
        fs::write(f.root.join(".ai-env/config.json"),serde_json::to_vec(&json!({"checks":{"test":{"argv":["python3","-m","unittest","discover","-s","tests"],"timeout_seconds":10},"fail":{"argv":["python3","-c","raise SystemExit(3)"],"timeout_seconds":10}}})).unwrap()).unwrap();
        f
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_aienv"));
        c.args(["--root", self.root.to_str().unwrap()]);
        c
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn okay(&self, args: &[&str]) -> Value {
        let o = self.run(args);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        serde_json::from_slice(&o.stdout).unwrap()
    }
    fn patch(&self) -> PathBuf {
        let snapshot = Project::open(&self.root).unwrap().snapshot().unwrap();
        let path = self.base.join("patch.json");
        fs::write(&path,serde_json::to_vec(&json!({"snapshot":snapshot.hash,"edits":[{"path":"inventory.py","expected_hash":sha(fs::read(self.root.join("inventory.py")).unwrap()),"old":"return stock","new":"return stock + allocated"}]})).unwrap()).unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
fn find_task<'a>(handoff: &'a Value, id: &str) -> &'a Value {
    handoff["tasks"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .unwrap()
}

#[test]
fn actual_multimodule_fix_requires_claim_current_proof_and_preserves_resume_state() {
    let f = Fixture::new();
    let before = fs::read(f.root.join("orders.py")).unwrap();
    f.okay(&[
        "task",
        "add",
        "refund",
        "--title",
        "Correct partial cancellation",
        "--paths",
        "inventory.py",
    ]);
    f.okay(&["task", "claim", "refund", "--owner", "worker-a"]);
    let failed = f.run(&["verify", "test"]);
    assert_eq!(failed.status.code(), Some(1));
    let failed: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(failed["passed"], false);
    assert!(!f
        .run(&[
            "task",
            "complete",
            "refund",
            "--owner",
            "worker-a",
            "--proof",
            failed["id"].as_str().unwrap()
        ])
        .status
        .success());
    let patch = f.patch();
    assert!(!f
        .run(&["apply", "--patch", patch.to_str().unwrap()])
        .status
        .success());
    let result = f.okay(&[
        "apply",
        "--patch",
        patch.to_str().unwrap(),
        "--task",
        "refund",
        "--owner",
        "worker-a",
    ]);
    assert_eq!(result["applied"], true);
    assert_eq!(fs::read(f.root.join("orders.py")).unwrap(), before);
    assert!(!f
        .run(&[
            "apply",
            "--patch",
            patch.to_str().unwrap(),
            "--task",
            "refund",
            "--owner",
            "worker-a"
        ])
        .status
        .success());
    let proof = f.okay(&["verify", "test"]);
    assert_eq!(proof["passed"], true);
    assert!(proof["stdout"].is_string());
    f.okay(&[
        "task",
        "complete",
        "refund",
        "--owner",
        "worker-a",
        "--proof",
        proof["id"].as_str().unwrap(),
    ]);
    f.okay(&[
        "note",
        "set",
        "refund-rule",
        "--text",
        "Cancellation returns all reserved units",
        "--paths",
        "inventory.py",
    ]);
    let handoff = f.okay(&["handoff"]);
    assert_eq!(find_task(&handoff, "refund")["status"], "done");
    assert_eq!(handoff["decisions"]["notes"][0]["stale"], false);
    fs::write(f.root.join("unrelated.py"), "answer = 42\n").unwrap();
    let handoff = f.okay(&["handoff"]);
    assert_eq!(find_task(&handoff, "refund")["status"], "done");
    assert!(handoff["tasks"]["fresh_proofs"]
        .as_array()
        .unwrap()
        .is_empty());
    fs::write(
        f.root.join("inventory.py"),
        "def refund(stock, allocated):\n    return stock\n",
    )
    .unwrap();
    let handoff = f.okay(&["handoff"]);
    assert_eq!(find_task(&handoff, "refund")["status"], "stale");
    assert_eq!(handoff["decisions"]["notes"][0]["stale"], true);
}

#[test]
fn two_actual_processes_cannot_claim_overlapping_scopes() {
    let f = Fixture::new();
    for id in ["a", "b"] {
        f.okay(&[
            "task",
            "add",
            id,
            "--title",
            "Review shared file",
            "--paths",
            "inventory.py",
        ]);
    }
    let project = Project::open(&f.root).unwrap();
    let lock = project.lock().unwrap();
    let first = f
        .command()
        .args(["task", "claim", "a", "--owner", "one"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let second = f
        .command()
        .args(["task", "claim", "b", "--owner", "two"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(lock);
    let a = first.wait_with_output().unwrap();
    let b = second.wait_with_output().unwrap();
    assert_eq!(
        usize::from(a.status.success()) + usize::from(b.status.success()),
        1
    );
    let state = f.okay(&["status"]);
    assert_eq!(
        state["tasks"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["status"] == "running")
            .count(),
        1
    );
}

#[test]
fn failed_command_exit_is_recorded_and_does_not_change_source() {
    let f = Fixture::new();
    let before = fs::read(f.root.join("inventory.py")).unwrap();
    let result = f.run(&["verify", "fail"]);
    assert_eq!(result.status.code(), Some(1));
    let proof: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(proof["passed"], false);
    assert_eq!(proof["exit_code"], 3);
    assert!(f
        .root
        .join(".ai-env/proofs")
        .join(format!("{}.json", proof["id"].as_str().unwrap()))
        .is_file());
    assert_eq!(before, fs::read(f.root.join("inventory.py")).unwrap());
}

#[test]
fn initial_sources_remain_unchanged_and_invalid_roots_fail_actionably() {
    let f = Fixture::new();
    let before = fs::read(f.root.join("inventory.py")).unwrap();
    f.okay(&["init"]);
    assert_eq!(before, fs::read(f.root.join("inventory.py")).unwrap());
    let missing = f.base.join("empty");
    fs::create_dir(&missing).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_aienv"))
        .args(["--root", missing.to_str().unwrap(), "status"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("init"));
}

#[test]
fn resume_context_stays_bounded_without_hiding_that_details_are_omitted() {
    let f = Fixture::new();
    let p = Project::open(&f.root).unwrap();
    for i in 0..24 {
        let args = vec![
            "add".into(),
            format!("job-{i:02}"),
            "--title".into(),
            "Long project task ".repeat(45),
            "--paths".into(),
            format!("module-{i}.py"),
        ];
        aienv::tasks::run(&p, &args).unwrap();
    }
    for i in 0..12 {
        let args = vec![
            "set".into(),
            format!("decision-{i:02}"),
            "--text".into(),
            "Ω".repeat(1500),
            "--paths".into(),
            "inventory.py".into(),
        ];
        aienv::notes::run(&p, &args).unwrap();
    }
    let output = f.run(&["handoff"]);
    assert!(output.status.success());
    assert!(output.stdout.len() <= 16000);
    let state: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(state["tasks"]["task_count"], 24);
    assert!(state["tasks"]["omitted_tasks"].as_u64().unwrap() > 0);
    assert_eq!(state["decisions"]["note_count"], 12);
    assert!(state["decisions"]["omitted_notes"].as_u64().unwrap() > 0);
    let note = f.okay(&["note", "show", "decision-00"]);
    assert_eq!(note["note"]["text"].as_str().unwrap(), "Ω".repeat(1500));
}

#[test]
fn stdout_preserves_the_exact_context_budget_without_an_extra_newline() {
    let f = Fixture::new();
    fs::write(
        f.root.join("long.py"),
        "# a meaningful source line\n".repeat(60),
    )
    .unwrap();
    let initial = f.run(&["context", "--path", "long.py", "--budget", "32000"]);
    assert!(initial.status.success());
    let text = String::from_utf8(initial.stdout).unwrap();
    let exact = text.trim_end_matches('\n').len() + 1;
    assert!(exact >= 1024);
    let result = f.run(&[
        "context",
        "--path",
        "long.py",
        "--budget",
        &exact.to_string(),
    ]);
    assert!(result.status.success());
    assert_eq!(result.stdout.len(), exact);
    assert!(result.stdout.ends_with(b"end\n"));
    assert!(!result.stdout.ends_with(b"end\n\n"));
}

#[test]
fn handoff_resumes_durable_workflow_without_replaying_model_context() {
    let f = Fixture::new();
    let request = f.base.join("workflow-request.txt");
    fs::write(
        &request,
        "Correct inventory refunds without changing public callers",
    )
    .unwrap();
    f.okay(&[
        "workflow",
        "start",
        "refund-flow",
        "--request",
        request.to_str().unwrap(),
        "--mode",
        "single",
    ]);
    let snapshot = Project::open(&f.root).unwrap().snapshot().unwrap().hash;
    let report = f.base.join("workflow-plan.json");
    fs::write(&report, serde_json::to_vec(&json!({
        "snapshot":snapshot,
        "tasks":[{"id":"refund","title":"Correct refund","instructions":"Reuse inventory helper", "paths":["inventory.py"],"context":["orders.py"],"depends":[]}],
        "checks":["test"],"acceptance":["Cancellation restores allocated stock"]
    })).unwrap()).unwrap();
    f.okay(&[
        "workflow",
        "plan",
        "refund-flow",
        "--file",
        report.to_str().unwrap(),
    ]);
    let output = f.run(&["handoff"]);
    assert!(output.status.success());
    assert!(output.stdout.len() <= 16000);
    let handoff: Value = serde_json::from_slice(&output.stdout).unwrap();
    let workflow = &handoff["workflows"]["workflows"][0];
    assert_eq!(workflow["id"], "refund-flow");
    assert_eq!(workflow["phase"], "developing");
    assert_eq!(workflow["task_counts"]["pending"], 1);
    assert_eq!(workflow["attempts"], 0);
    assert_eq!(workflow["details"], "workflow status refund-flow");
    assert!(workflow.get("task_states").is_none());
}
