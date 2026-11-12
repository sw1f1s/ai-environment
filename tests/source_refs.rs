use aienv::{
    brief,
    project::{sha, Project, Snapshot},
    source_refs,
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
            "ae-source-refs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let project = Project::init(&root).unwrap();
        let f = Self { root, project };
        f.write("src/core.py", "def change(value):\r\n    return '雪😀'\r\n");
        f.write(
            "helpers/policy.py",
            "def allows(value):\n    return value <= 7\n",
        );
        f.write("outside.py", "outside = True\n");
        f
    }
    fn write(&self, path: &str, text: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn snapshot(&self) -> Snapshot {
        self.project.snapshot().unwrap()
    }
    fn state(&self, snapshot: &Snapshot) -> Value {
        json!({"snapshot":snapshot.hash,"goal":"Delegate core changes to the authoritative policy", "acceptance":["Core delegates to authoritative policy"],
            "tasks":[{"id":"change","title":"Change core","instructions":"Reuse the existing policy and preserve Unicode", "paths":["src"],"context":["helpers/policy.py"],"depends":[]}],
            "verification":[{"id":"core","acceptance":[0],"tasks":["change"],"paths":["src/core.py"],"risk":"Preserve Unicode and error ordering","checks":[]},
                {"id":"policy","acceptance":[0],"tasks":["change"],"paths":["helpers/policy.py"],"risk":"Reuse the original policy","checks":[]}]})
    }
    fn packet(&self, args: &[&str]) -> String {
        brief::run(
            &self.project,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap()
    }
    fn manifest(&self) -> (Snapshot, Value, Value) {
        let snapshot = self.snapshot();
        let manifest =
            source_refs::capture(&snapshot, &self.state(&snapshot), &self.packet(&[])).unwrap();
        let public = source_refs::public(&manifest).unwrap();
        (snapshot, manifest, public)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn answer(snapshot: &str, public: &Value) -> Value {
    let rows = ["core", "policy"].iter().map(|id|json!({"id":id,"status":"inspected","proofs":[],
            "source_refs":public["by_obligation"][*id][0].as_str().into_iter().collect::<Vec<_>>(),"evidence":"The supplied source was inspected; no execution claimed"})).collect::<Vec<_>>();
    json!({"snapshot":snapshot,"verdict":"accept","proofs":[],"verification":rows})
}

#[test]
fn real_full_brief_cards_ground_unicode_crlf_and_exclude_unscoped_source() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let packet = f.packet(&["--hashes"]);
    let manifest = source_refs::capture(&snapshot, &f.state(&snapshot), &packet).unwrap();
    let public = source_refs::public(&manifest).unwrap();
    assert_eq!(public["refs"].as_array().unwrap().len(), 2);
    let card = public["refs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["path"] == "src/core.py")
        .unwrap();
    assert_eq!(card["line"], 1);
    assert_eq!(card["end_line"], 2);
    assert_eq!(card["obligations"], json!(["core"]));
    assert!(public["refs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["id"].as_str().unwrap().len() <= 12));
    assert_eq!(public["excluded_fragments"], 1);
    for private in [
        "contract_hash",
        "file_hash",
        "text_hash",
        "source_hash",
        "digest",
    ] {
        assert!(!public.to_string().contains(private));
    }
    assert!(!public
        .to_string()
        .contains(&sha(fs::read(f.root.join("src/core.py")).unwrap())));
    let bound =
        source_refs::bind(&answer(&snapshot.hash, &public), &manifest, &snapshot.hash).unwrap();
    assert_eq!(
        bound["verification"][0]["locations"],
        json!([{"path":"src/core.py","line":1}])
    );
    assert!(bound["verification"][0].get("source_refs").is_none());
    assert_eq!(bound["verdict"], "accept");
}

#[test]
fn consistency_metadata_never_becomes_visible_source_evidence() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let source = f.packet(&["--impact", "change"]);
    assert!(source.contains("implementation-candidate @src/core.py"));
    let base = source_refs::capture(&snapshot, &f.state(&snapshot), &source).unwrap();
    let public = source_refs::public(&base).unwrap();
    assert_eq!(public["refs"].as_array().unwrap().len(), 2);
    let card = public["refs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"] == "src/core.py")
        .unwrap();
    assert_eq!(card["end_line"], 2);
    // A navigation-only row must never authorize an absent code fragment.
    let metadata_only = format!(
        "snapshot {}\nimplementation-candidate @src/core.py:1 (line-missing): query match\n",
        snapshot.hash
    );
    let absent = source_refs::capture(&snapshot, &f.state(&snapshot), &metadata_only).unwrap();
    assert!(source_refs::public(&absent).unwrap()["refs"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn numbered_excerpt_runs_only_reference_exact_visible_original_lines() {
    let f = Fixture::new();
    f.write(
        "src/core.py",
        &(1..=100)
            .map(|n| format!("line_{n} = '雪😀'\r\n"))
            .collect::<String>(),
    );
    let snapshot = f.snapshot();
    let source=format!("snapshot {}\n\n[src/core.py excerpt; omitted lines require source inspection]\n10:line_10 = '雪😀'\n11:line_11 = '雪😀'\n40:line_40 = '雪😀'\n\ncoverage omitted\n",snapshot.hash);
    let manifest = source_refs::capture(&snapshot, &f.state(&snapshot), &source).unwrap();
    let public = source_refs::public(&manifest).unwrap();
    let cards = public["refs"].as_array().unwrap();
    assert_eq!(cards.len(), 2);
    assert_eq!(
        (cards[0]["line"].as_u64(), cards[0]["end_line"].as_u64()),
        (Some(10), Some(11))
    );
    assert_eq!(
        (cards[1]["line"].as_u64(), cards[1]["end_line"].as_u64()),
        (Some(40), Some(40))
    );
    assert_eq!(public["by_obligation"]["policy"], json!([]));
    assert_eq!(public["obligations_without_refs"], json!(["policy"]));
    let mut report = answer(&snapshot.hash, &public);
    report["verification"][0]["source_refs"] = json!([cards[1]["id"]]);
    report["verification"][1]["status"] = json!("gap");
    report["verdict"] = json!("changes");
    let bound = source_refs::bind(&report, &manifest, &snapshot.hash).unwrap();
    assert_eq!(
        bound["verification"][0]["locations"],
        json!([{"path":"src/core.py","line":40}])
    );
    assert!(bound["verification"][1]["locations"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn actual_budgeted_brief_excerpt_and_missing_final_newline_are_supported() {
    let f = Fixture::new();
    f.write(
        "src/core.py",
        &(1..=500)
            .map(|line| {
                format!("change_{line} = '雪😀 long enough to require excerpt selection'\r\n")
            })
            .collect::<String>(),
    );
    let snapshot = f.snapshot();
    let packet = f.packet(&[
        "--budget",
        "2048",
        "--hashes",
        "--path",
        "src/core.py",
        "--",
        "change_400",
    ]);
    assert!(packet.contains(" excerpt hash="));
    let catalog = source_refs::capture(&snapshot, &f.state(&snapshot), &packet).unwrap();
    let visible = source_refs::public(&catalog).unwrap();
    assert!(!visible["by_obligation"]["core"]
        .as_array()
        .unwrap()
        .is_empty());
    for card in visible["refs"].as_array().unwrap() {
        let start = card["line"].as_u64().unwrap() as usize;
        let end = card["end_line"].as_u64().unwrap() as usize;
        assert!(start > 1);
        for line in start..=end {
            assert!(packet.contains(&format!("\n{line}:change_{line} = '雪😀")));
        }
    }
    f.write("src/core.py", "value = '雪😀'");
    let snapshot = f.snapshot();
    let packet = f.packet(&["--path", "src/core.py"]);
    let catalog = source_refs::capture(&snapshot, &f.state(&snapshot), &packet).unwrap();
    let visible = source_refs::public(&catalog).unwrap();
    assert_eq!(visible["refs"][0]["line"], 1);
    assert_eq!(visible["refs"][0]["end_line"], 1);
}

#[test]
fn header_looking_source_text_cannot_create_a_reference_to_another_file() {
    let f = Fixture::new();
    f.write(
        "src/core.py",
        "message = '''\n[helpers/policy.py:1..2]\ndef allows(value):\n    return value <= 7\n'''\n",
    );
    let snapshot = f.snapshot();
    let packet = f.packet(&["--path", "src/core.py"]);
    let manifest = source_refs::capture(&snapshot, &f.state(&snapshot), &packet).unwrap();
    let public = source_refs::public(&manifest).unwrap();
    assert_eq!(public["refs"].as_array().unwrap().len(), 1);
    assert_eq!(public["refs"][0]["path"], "src/core.py");
    assert_eq!(public["by_obligation"]["policy"], json!([]));
}

#[test]
fn malformed_spoofed_or_nonindexed_source_blocks_fail_closed() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let state = f.state(&snapshot);
    let bad=[
        "[src/core.py:1..2]\nnot actual source\n",
        "[src/core.py:1..1]\ndef change(value):\n",
        "[src/missing.py:1..1]\nforged\n",
        "[src/core.py:1..02]\ndef change(value):\n",
        "[src/core.py excerpt; omitted lines require source inspection]\n0:forged\n",
        "[src/core.py excerpt; omitted lines require source inspection]\n3:forged\n",
        "[src/core.py excerpt; omitted lines require source inspection]\n1:forged\n",
        "[src/core.py excerpt; omitted lines require source inspection]\n01:def change(value):\n",
        "[src/core.py excerpt; omitted lines require source inspection]\n1:def change(value):\n1:def change(value):\n",
        "[src/core.py excerpt; omitted lines require source inspection]\n2:    return '雪😀'\n1:def change(value):\n",
        "[src/core.py excerpt; omitted lines require source inspection]\n1:def change(value):\nmalformed second row\n",
        "[src/core.py excerpt; omitted lines require source inspection]\n\n",
        "[src/core.py excerpt hash=invalid; omitted lines require source inspection]\n1:def change(value):\n",
    ];
    for fragment in bad {
        assert!(
            source_refs::capture(
                &snapshot,
                &state,
                &format!("snapshot {}\n\n{fragment}", snapshot.hash)
            )
            .is_err(),
            "accepted {fragment:?}"
        );
    }
    let packet = f.packet(&["--path", "src/core.py"]);
    assert!(source_refs::capture(
        &snapshot,
        &state,
        &format!("{packet}\n[src/core.py:1..2]\ndef change(value):\r\n    return '雪😀'\r\n")
    )
    .is_err());
    let wrong_hash=format!("snapshot {}\n\n[src/core.py excerpt hash={}; omitted lines require source inspection]\n1:def change(value):\n\n",snapshot.hash,"0".repeat(64));
    assert!(source_refs::capture(&snapshot, &state, &wrong_hash).is_err());
    assert!(source_refs::capture(
        &snapshot,
        &state,
        &packet.replacen(&snapshot.hash, &"0".repeat(64), 1)
    )
    .is_err());
}

#[test]
fn contract_requires_both_obligation_and_responsible_task_scope() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let mut state = f.state(&snapshot);
    // Merely naming a helper in the obligation does not make it part of an
    // unrelated responsible task's read/write scope.
    state["tasks"][0]["context"] = json!([]);
    let manifest = source_refs::capture(&snapshot, &state, &f.packet(&[])).unwrap();
    let public = source_refs::public(&manifest).unwrap();
    assert_eq!(public["by_obligation"]["policy"], json!([]));
    assert_eq!(public["refs"].as_array().unwrap().len(), 1);
    state["verification"][0]["tasks"] = json!(["unknown"]);
    assert!(source_refs::capture(&snapshot, &state, &f.packet(&[])).is_err());
}

#[test]
fn native_answers_reject_paths_guessed_duplicate_wrong_owner_and_missing_references() {
    let f = Fixture::new();
    let (snapshot, manifest, public) = f.manifest();
    let valid = answer(&snapshot.hash, &public);
    for status in ["verified", "inspected"] {
        let mut report = valid.clone();
        report["verification"][0]["status"] = json!(status);
        report["verification"][0]["source_refs"] = json!([]);
        assert!(source_refs::bind(&report, &manifest, &snapshot.hash).is_err());
    }
    let mut bad = Vec::new();
    let mut r = valid.clone();
    r["verification"][0]["locations"] = json!([{ "path":"src/core.py","line":1}]);
    bad.push(r);
    let mut r = valid.clone();
    r["verification"][0]["source_refs"] = json!(["sunknown"]);
    bad.push(r);
    let mut r = valid.clone();
    r["verification"][0]["source_refs"] = json!([public["by_obligation"]["policy"][0]]);
    bad.push(r);
    let mut r = valid.clone();
    r["verification"][0]["source_refs"] = json!([
        public["by_obligation"]["core"][0],
        public["by_obligation"]["core"][0]
    ]);
    bad.push(r);
    let mut r = valid.clone();
    r["verification"][0]["source_refs"] = json!([{ "path":"src/core.py","line":1}]);
    bad.push(r);
    let mut r = valid.clone();
    r["verification"][0]
        .as_object_mut()
        .unwrap()
        .remove("source_refs");
    bad.push(r);
    let mut r = valid.clone();
    r["verification"][0]["id"] = json!("unknown");
    bad.push(r);
    let mut r = valid.clone();
    r["verification"][1] = r["verification"][0].clone();
    bad.push(r);
    let mut r = valid.clone();
    r["verification"][0]["unused"] = json!(true);
    bad.push(r);
    let mut r = valid.clone();
    r["verification"] = json!([]);
    bad.push(r);
    for report in bad {
        assert!(
            source_refs::bind(&report, &manifest, &snapshot.hash).is_err(),
            "accepted {report}"
        );
    }
    let mut gap = valid.clone();
    gap["verification"][0]["status"] = json!("gap");
    gap["verification"][0]["source_refs"] = json!([]);
    gap["verdict"] = json!("changes");
    assert!(source_refs::bind(&gap, &manifest, &snapshot.hash).is_ok());
    // Source grounding must not manufacture proof execution or alter evidence.
    let mut checked = valid.clone();
    checked["verification"][0]["status"] = json!("verified");
    checked["verification"][0]["proofs"] = json!(["actual-proof-id"]);
    let bound = source_refs::bind(&checked, &manifest, &snapshot.hash).unwrap();
    assert_eq!(
        bound["verification"][0]["proofs"],
        json!(["actual-proof-id"])
    );
}

#[test]
fn catalog_corruption_and_replay_never_recapture_current_source() {
    let f = Fixture::new();
    let (snapshot, manifest, public) = f.manifest();
    let report = answer(&snapshot.hash, &public);
    f.project
        .write_state("source-catalog.json", &manifest)
        .unwrap();
    f.write("src/core.py", "external_writer = True\n");
    let resumed = Project::open(&f.root).unwrap();
    let saved: Value = resumed.read_state("source-catalog.json").unwrap();
    assert!(
        source_refs::bind(&report, &saved, &snapshot.hash).is_ok(),
        "Binding uses original persisted locations; controller checks current source separately"
    );
    assert!(source_refs::bind(&report, &saved, &resumed.snapshot().unwrap().hash).is_err());
    for (key, value) in [
        ("digest", json!("0".repeat(64))),
        ("snapshot", json!("0".repeat(64))),
        ("schema", json!(2)),
        ("unexpected", json!(true)),
    ] {
        let mut bad = saved.clone();
        bad[key] = value;
        assert!(source_refs::public(&bad).is_err());
        assert!(source_refs::bind(&report, &bad, &snapshot.hash).is_err());
    }
    let mut bad = saved.clone();
    bad["refs"][0]["line"] = json!(999);
    assert!(source_refs::public(&bad).is_err());
    let mut bad = saved.clone();
    bad["contract"]["obligations"][0]["risk"] = json!("different contract");
    assert!(source_refs::public(&bad).is_err());
    assert!(source_refs::bind(&report, &Value::Null, &snapshot.hash).is_err());
    // Same source snapshot, but a different contract must not reuse card IDs.
    f.write("src/core.py", "def change(value):\r\n    return '雪😀'\r\n");
    let original = f.snapshot();
    assert_eq!(original.hash, snapshot.hash);
    let mut state = f.state(&original);
    state["verification"][0]["risk"] = json!("Different required semantic boundary");
    let mixed = source_refs::capture(&original, &state, &f.packet(&[])).unwrap();
    assert!(source_refs::bind(&report, &mixed, &snapshot.hash).is_err());
}

#[test]
fn empty_or_missing_visible_source_never_creates_a_synthetic_line_one() {
    let f = Fixture::new();
    f.write("src/core.py", "");
    let snapshot = f.snapshot();
    for packet in [
        f.packet(&["--path", "src/core.py"]),
        format!("snapshot {}\ncoverage no source supplied\n", snapshot.hash),
    ] {
        let manifest = source_refs::capture(&snapshot, &f.state(&snapshot), &packet).unwrap();
        let public = source_refs::public(&manifest).unwrap();
        assert!(public["refs"].as_array().unwrap().is_empty());
        assert_eq!(public["by_obligation"]["core"], json!([]));
        assert!(
            source_refs::bind(&answer(&snapshot.hash, &public), &manifest, &snapshot.hash).is_err()
        );
    }
}

#[test]
fn omitted_reference_cards_stay_bounded_explicit_and_unusable() {
    let f = Fixture::new();
    f.write("src/core.py", &"x\n".repeat(410));
    let snapshot = f.snapshot();
    let state = f.state(&snapshot);
    let mut packet = format!(
        "snapshot {}\n\n[src/core.py excerpt; omitted lines require source inspection]\n",
        snapshot.hash
    );
    for line in (1..=401).step_by(2) {
        packet.push_str(&format!("{line}:x\n"));
    }
    packet.push_str("\n[helpers/policy.py:1..2]\ndef allows(value):\n    return value <= 7\n");
    let manifest = source_refs::capture(&snapshot, &state, &packet).unwrap();
    let public = source_refs::public(&manifest).unwrap();
    assert!(serde_json::to_vec(&public).unwrap().len() <= 16 * 1024);
    assert!(public["refs"].as_array().unwrap().len() <= 128);
    assert!(public["omitted_refs"].as_u64().unwrap() > 0);
    assert!(!public["by_obligation"]["policy"]
        .as_array()
        .unwrap()
        .is_empty());
    let last = format!(
        "snapshot {}\n\n[src/core.py excerpt; omitted lines require source inspection]\n401:x\n\n",
        snapshot.hash
    );
    let last =
        source_refs::public(&source_refs::capture(&snapshot, &state, &last).unwrap()).unwrap();
    let mut report = answer(&snapshot.hash, &public);
    report["verification"][0]["source_refs"] = json!([last["refs"][0]["id"]]);
    assert!(source_refs::bind(&report, &manifest, &snapshot.hash).is_err());
}

#[test]
fn unchanged_source_cannot_reuse_evidence_after_contract_changes() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let mut state = f.state(&snapshot);
    state["required_checks"] = json!(["test"]);
    let manifest = source_refs::capture(&snapshot, &state, &f.packet(&[])).unwrap();
    source_refs::validate_contract(&manifest, &state).unwrap();
    let mut progress = state.clone();
    progress["phase"] = json!("qa");
    progress["round"] = json!(3);
    progress["completed"] = json!(["change"]);
    progress["proofs"] = json!(["new-proof"]);
    source_refs::validate_contract(&manifest, &progress).unwrap();
    for changed in [
        {
            let mut changed = state.clone();
            changed["verification"][0]["risk"] = json!("A different semantic boundary");
            changed
        },
        {
            let mut changed = state.clone();
            changed["tasks"][0]["context"] = json!(["helpers"]);
            changed
        },
        {
            let mut changed = state.clone();
            changed["tasks"][0]["paths"] = json!(["src/core.py"]);
            changed
        },
        {
            let mut changed = state.clone();
            changed["verification"][0]["paths"] = json!(["src"]);
            changed
        },
        {
            let mut changed = state.clone();
            changed["acceptance"][0] = json!("Different acceptance");
            changed
        },
        {
            let mut changed = state.clone();
            changed["required_checks"] = json!(["other-check"]);
            changed
        },
    ] {
        assert_eq!(changed["snapshot"], snapshot.hash);
        assert!(source_refs::validate_contract(&manifest, &changed).is_err());
    }
    assert_eq!(f.snapshot().hash, snapshot.hash);
}

#[test]
fn unchanged_source_rejects_semantic_task_architecture_and_request_replans() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let mut state = f.state(&snapshot);
    state["tasks"].as_array_mut().unwrap().push(json!({
        "id":"policy-owner","title":"Maintain policy", "instructions":"Keep policy authoritative",
        "paths":["helpers/policy.py"],"context":[],"depends":[]
    }));
    state["architecture"] = json!({"boundaries":["Core delegates policy"],"dependencies":["Core may read policy"],"extension_scenarios":[]});
    state["checks"] = json!(["unit"]);
    let manifest = source_refs::capture(&snapshot, &state, &f.packet(&[])).unwrap();
    for (field, value) in [
        ("title", json!("Change ownership")),
        ("instructions", json!("Inline policy rules into core")),
        ("depends", json!(["policy-owner"])),
    ] {
        let mut changed = state.clone();
        changed["tasks"][0][field] = value;
        assert!(
            source_refs::validate_contract(&manifest, &changed).is_err(),
            "accepted task {field} replan"
        );
    }
    for (field, value) in [
        ("goal", json!("A different original request")),
        ("architecture", Value::Null),
        (
            "architecture",
            json!({"boundaries":["Core owns policy"],"dependencies":[],"extension_scenarios":[]}),
        ),
        ("checks", json!(["integration"])),
    ] {
        let mut changed = state.clone();
        changed[field] = value;
        assert!(
            source_refs::validate_contract(&manifest, &changed).is_err(),
            "accepted {field} replan"
        );
    }
    let mut runtime = state.clone();
    runtime["phase"] = json!("done");
    runtime["proofs"] = json!(["proof"]);
    runtime["architecture_report"] = json!({"assessment":["new evidence"]});
    runtime["task_states"] = json!({"change":{"status":"completed"}});
    source_refs::validate_contract(&manifest, &runtime).unwrap();
    assert_eq!(f.snapshot().hash, snapshot.hash);
}
