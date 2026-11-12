use aienv::{
    project::{sha, Project},
    qa_context,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
    project: Project,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ae-qa-context-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let project = Project::init(&root).unwrap();
        let f = Self { root, project };
        f.write(
            "src/core.py",
            "from helpers.policy import allows\ndef change(value):\n    return allows(value)\n",
        );
        f.write(
            "helpers/policy.py",
            "def allows(value):\n    return value <= 7  # AUTHORITATIVE_POLICY\n",
        );
        f.write(
            "check.py",
            "print('prepared check driver; not executed by context preparation')\n",
        );
        f.write(".ai-env/config.json", r#"{"checks":{"test":{"argv":["python3","check.py"],"timeout_seconds":30},"unapproved":{"argv":["UNAPPROVED_COMMAND_SENTINEL"],"timeout_seconds":20}}}"#);
        f
    }
    fn write(&self, path: &str, source: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }
    fn state(&self) -> Value {
        json!({"snapshot":self.project.snapshot().unwrap().hash,"goal":"Preserve the library while extending an unrelated future facility",
            "acceptance":["change delegates to allows; preserve unchanged policy and Unicode КвотаРезерва"],"required_checks":["test"],
            "tasks":[{"id":"change","paths":["src"],"context":["helpers/policy.py"]},
                {"id":"unrelated","paths":["unrelated"],"context":["unrelated/architecture.py"]}],
            "verification":[{"id":"policy-owner","acceptance":[0],"tasks":["change"],"paths":["src/core.py"],
                "risk":"Reject the limit boundary through allows without copying policy","checks":["test"]}]})
    }
    fn build(&self, state: &Value, changed: &[&str], budget: usize) -> aienv::Result<Value> {
        let _lock = self.project.lock()?;
        qa_context::build(
            &self.project,
            state,
            &changed.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            budget,
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn qa_attaches_current_change_and_unchanged_owner_without_global_inventory_or_execution() {
    let f = Fixture::new();
    f.write("src/core.py", "from helpers.policy import allows\ndef change(value):\n    return allows(value + 1)  # CURRENT_CHANGE\n");
    for i in 0..120 {
        f.write(
            &format!("tests/test_unrelated_{i:03}.py"),
            "import unittest\n# GLOBAL_INVENTORY_SENTINEL\nclass Tests(unittest.TestCase): pass\n",
        );
    }
    f.write(
        "unrelated/architecture.py",
        &"UNRELATED_MODULE_SENTINEL\n".repeat(4000),
    );
    let value = f.build(&f.state(), &["src/core.py"], 12000).unwrap();
    let source = value["source"].as_str().unwrap();
    assert!(source.contains("CURRENT_CHANGE") && source.contains("AUTHORITATIVE_POLICY"));
    assert!(source.contains("prepared check driver"));
    let full = value.to_string();
    for omitted in [
        "GLOBAL_INVENTORY_SENTINEL",
        "test_unrelated_000.py",
        "UNRELATED_MODULE_SENTINEL",
    ] {
        assert!(
            !full.contains(omitted),
            "Attached unrelated inventory: {omitted}"
        );
    }
    assert!(!value["profile"]
        .to_string()
        .contains("UNAPPROVED_COMMAND_SENTINEL"));
    assert!(source.starts_with(&format!(
        "snapshot {}\n",
        f.state()["snapshot"].as_str().unwrap()
    )));
    assert_eq!(value["profile"]["coverage"]["tests_found"], 120);
    assert_eq!(value["profile"]["coverage"]["candidates_found"], 1);
    assert_eq!(
        value["profile"]["testing"]["configured"][0]["argv"],
        json!(["python3", "check.py"])
    );
    assert!(value["profile"]["testing"].get("discovered").is_none());
    assert!(value["profile"]["testing"].get("candidates").is_none());
    assert!(value["profile"]["style"].get("observations").is_none());
    assert!(value["profile"]["testing"]["status"]
        .as_str()
        .unwrap()
        .contains("not execution evidence or coverage proof"));
    assert_eq!(value["focus"]["current_changed_count"], 1);
    assert_eq!(
        value["focus"]["selected_obligation_ids"],
        json!(["policy-owner"])
    );
    assert!(value["focus"]["query"]
        .as_str()
        .unwrap()
        .contains("КвотаРезерва"));
    assert!(!full.contains(&sha(fs::read(f.root.join("src/core.py")).unwrap())));
    assert!(
        !f.root.join(".ai-env/semantic").exists(),
        "Missing graphs must never be built implicitly"
    );
}

#[test]
fn scoped_configs_and_source_obey_independent_byte_budgets_with_unicode() {
    let f = Fixture::new();
    f.write(
        ".editorconfig",
        &format!("root=true\n[*]\n# {}\n", "雪😀".repeat(1000)),
    );
    f.write("src/.editorconfig", "[*.py]\nindent_size = 2\n");
    f.write(
        "unrelated/.clang-format",
        "UNRELATED_STYLE_SENTINEL: true\n",
    );
    f.write(
        "src/core.py",
        &format!(
            "# {}\ndef change(value):\n    return '雪😀'\n",
            "large unrelated source line\n".repeat(1000)
        ),
    );
    for budget in [2048, 8000, 12000, 32000] {
        let value = f.build(&f.state(), &["src/core.py"], budget).unwrap();
        let source_budget = (2 * budget / 3).clamp(2048, 8000);
        let profile_budget = (budget / 4).clamp(2048, 4000);
        assert!(value["source"].as_str().unwrap().len() <= source_budget);
        assert!(serde_json::to_vec(&value["profile"]).unwrap().len() <= profile_budget);
        assert_eq!(value["focus"]["source_budget"], source_budget);
        assert_eq!(value["focus"]["profile_budget"], profile_budget);
        let configs = value["profile"]["style"]["configs"].as_array().unwrap();
        assert!(configs.iter().any(|c| c["path"] == "src/.editorconfig"));
        assert!(configs.iter().all(|c| c.get("evidence").is_none()));
        assert!(!value.to_string().contains("UNRELATED_STYLE_SENTINEL"));
        assert!(configs
            .iter()
            .any(|c| c["content_omitted_bytes"].as_u64().unwrap() > 0));
    }
}

#[test]
fn overflow_reports_read_gaps_and_preserves_every_instruction_scope_without_broadening() {
    let f = Fixture::new();
    let changed = (0..90)
        .map(|i| format!("changed/item_{i:03}.py"))
        .collect::<Vec<_>>();
    for path in &changed {
        f.write(path, "value = 2\n");
    }
    let state = f.state();
    let value = f
        .build(
            &state,
            &changed.iter().map(String::as_str).collect::<Vec<_>>(),
            12000,
        )
        .unwrap();
    let selected = value["focus"]["selected_paths"].as_array().unwrap();
    assert_eq!(selected.len(), 64);
    assert!(
        selected
            .iter()
            .map(|p| p.as_str().unwrap().len())
            .sum::<usize>()
            <= 2048
    );
    assert!(
        selected.contains(&json!("src/core.py")),
        "Obligation path has first priority"
    );
    assert_eq!(value["paths"].as_array().unwrap().len(), 92);
    assert_eq!(value["focus"]["omitted_path_count"], 28);
    assert!(value["focus"]["omitted_paths"].as_array().unwrap().len() <= 16);
    assert!(!selected.contains(&json!(".")) && !selected.contains(&json!("changed")));
    assert_eq!(value["focus"]["total_paths"], 92);
    assert!(value["focus"]["status"]
        .as_str()
        .unwrap()
        .contains("read gaps"));
    let small = f
        .build(
            &state,
            &changed.iter().map(String::as_str).collect::<Vec<_>>(),
            2048,
        )
        .unwrap();
    assert!(small["source"].as_str().unwrap().len() <= 2048);
    assert!(small["focus"]["omitted_path_count"].as_u64().unwrap() > 28);
    assert!(!small["focus"]["selected_paths"]
        .as_array()
        .unwrap()
        .contains(&json!(".")));
}

#[test]
fn legacy_context_uses_changed_paths_and_goal_with_explicit_missing_source_gap() {
    let f = Fixture::new();
    let mut state = f.state();
    state["verification"] = json!([]);
    state["goal"] = json!("Check legacy_boundary and КвотаРезерва");
    let value = f
        .build(&state, &["src/core.py", "src/deleted.py"], 12000)
        .unwrap();
    assert!(value["source"]
        .as_str()
        .unwrap()
        .contains("AUTHORITATIVE_POLICY"));
    assert!(value["focus"]["query"]
        .as_str()
        .unwrap()
        .contains("legacy_boundary"));
    assert_eq!(value["focus"]["missing_paths"], json!(["src/deleted.py"]));
    assert_eq!(value["focus"]["current_changed_count"], 1);
    assert_eq!(value["focus"]["changed_path_count"], 2);
    let fallback = f.build(&state, &[], 12000).unwrap();
    assert!(fallback["focus"]["selected_paths"]
        .as_array()
        .unwrap()
        .contains(&json!("src")));
    state["tasks"] = json!([]);
    assert!(f
        .build(&state, &[], 12000)
        .unwrap_err()
        .to_string()
        .contains("no whole-project fallback"));
}

#[test]
fn explicit_prefix_dedup_and_byte_limit_never_invent_a_parent_scope() {
    let f = Fixture::new();
    let mut state = f.state();
    state["tasks"][0]["context"] = json!(["src", "src/core.py", "helpers/policy.py"]);
    let value = f.build(&state, &["src/core.py"], 12000).unwrap();
    assert_eq!(
        value["focus"]["selected_paths"],
        json!(["src", "helpers/policy.py"])
    );
    assert_eq!(value["focus"]["covered_path_count"], 3);
    assert_eq!(value["focus"]["omitted_path_count"], 0);
    let paths = (0..12)
        .map(|i| format!("{}_{i}.py", "a".repeat(200)))
        .collect::<Vec<_>>();
    state["tasks"][0]["context"] = json!(paths);
    let value = f.build(&state, &["src/core.py"], 12000).unwrap();
    assert!(value["focus"]["omitted_path_count"].as_u64().unwrap() > 0);
    assert!(value["focus"]["selected_paths"].as_array().unwrap().len() < 64);
    assert!(
        value["focus"]["selected_paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().len())
            .sum::<usize>()
            <= 2048
    );
}

#[test]
fn stale_state_invalid_scopes_and_acceptance_references_fail_closed() {
    let f = Fixture::new();
    let state = f.state();
    f.write("src/core.py", "changed_after_state = True\n");
    assert!(f.build(&state, &["src/core.py"], 12000).is_err());
    for path in [
        "../outside",
        "./src",
        "src/",
        "src//core.py",
        ".ai-env/state",
        "/src",
        "src\\core.py",
    ] {
        assert!(
            f.build(&f.state(), &[path], 12000).is_err(),
            "accepted {path}"
        );
    }
    let mut state = f.state();
    state["verification"][0]["acceptance"] = json!([4]);
    assert!(f.build(&state, &["src/core.py"], 12000).is_err());
}
