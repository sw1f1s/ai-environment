use aienv::{project::Project, verify, workflow};
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
            "ae-workflow-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        let project = Project::init(&root).unwrap();
        fs::write(root.join("src/a.py"), "a=1\n").unwrap();
        fs::write(root.join("src/b.py"), "b=1\n").unwrap();
        fs::write(
            root.join("request.txt"),
            "Implement a feature using project conventions",
        )
        .unwrap();
        Self { root, project }
    }
    fn run(&self, args: &[&str]) -> aienv::Result<Value> {
        Ok(serde_json::from_str(&workflow::run(
            &self.project,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )?)?)
    }
    fn start(&self, rounds: &str) {
        self.run(&[
            "start",
            "demo",
            "--request",
            self.root.join("request.txt").to_str().unwrap(),
            "--rounds",
            rounds,
        ])
        .unwrap();
    }
    fn snapshot(&self) -> String {
        self.project.snapshot().unwrap().hash
    }
    fn report(&self, action: &str, value: Value) -> aienv::Result<Value> {
        // Reports are external inputs, not project source: writing one must not
        // accidentally invalidate the snapshot it reports on.
        let path = self.root.with_extension("report.json");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let result = self.run(&[action, "demo", "--file", path.to_str().unwrap()]);
        fs::remove_file(path).unwrap();
        result
    }
    fn plan(&self, tasks: Value, checks: Value) -> aienv::Result<Value> {
        self.report("plan",json!({"snapshot":self.snapshot(),"tasks":tasks,"checks":checks,"acceptance":["Feature works"]}))
    }
    fn architecture_plan(&self, tasks: Value, architecture: Value) -> aienv::Result<Value> {
        self.report("plan", json!({"snapshot":self.snapshot(),"tasks":tasks,"checks":[],"acceptance":["Feature works"],"architecture":architecture}))
    }
    fn apply(&self, ids: &[(&str, &str)]) -> aienv::Result<Value> {
        let snapshot = self.project.snapshot().unwrap();
        let jobs=ids.iter().map(|(id,path)|json!({"id":id,"summary":"Implemented and tested", "files":[{"path":path,"expected_hash":snapshot.files[*path].hash,"source":format!("{}# edited\n",snapshot.files[*path].text)}]})).collect::<Vec<_>>();
        self.report("apply", json!({"snapshot":snapshot.hash,"jobs":jobs}))
    }
}
fn architecture_contract() -> Value {
    json!({"boundaries":["Pricing owns monetary calculations; service owns reservation mutations."],
        "dependencies":["Router delegates state changes to the service."],
        "extension_scenarios":[{"change":"Add another pricing policy without changing inventory storage.","expected_paths":["src/pricing"],"preserved_paths":["src/a.py"]}]})
}
fn architecture_issue(task: &str, path: &str, severity: &str) -> Value {
    json!({"severity":severity,"tasks":[task],"path":path,"line":1,
        "problem":"The inventory module constructs a carrier dependency.",
        "consequence":"Changing a carrier requires changing inventory state code.",
        "fix":"Move carrier selection behind the existing service boundary.",
        "verification":"Run reservation regressions with two carrier implementations."})
}
fn architecture_assessment(issues: Value) -> Value {
    json!({"assessment":["The source boundary and actual service callers were inspected; no claim of exhaustive coverage."],"issues":issues})
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn job(id: &str, path: &str, deps: &[&str]) -> Value {
    json!({"id":id,"title":id,"instructions":"Implement small scoped change", "paths":[path],"depends":deps})
}
fn obligation(
    id: &str,
    acceptance: &[usize],
    tasks: &[&str],
    paths: &[&str],
    checks: &[&str],
) -> Value {
    json!({"id":id,"acceptance":acceptance,"tasks":tasks,"paths":paths,
        "risk":"A rejected boundary request must preserve the previous state.","checks":checks})
}
fn verification_result(id: &str, status: &str, proofs: Value, path: &str) -> Value {
    json!({"id":id,"status":status,"proofs":proofs,"locations":[{"path":path,"line":1}],
        "evidence":"Static-only source inspection identifies the rejection branch before mutation; execution coverage is not inferred."})
}
impl Fixture {
    fn verification_plan(
        &self,
        tasks: Value,
        checks: Value,
        acceptance: Value,
        obligations: Value,
    ) -> aienv::Result<Value> {
        self.report(
            "plan",
            json!({"snapshot":self.snapshot(),"tasks":tasks,"checks":checks,
            "acceptance":acceptance,"verification":obligations}),
        )
    }
    fn accepted_review(&self) {
        self.report(
            "review",
            json!({"snapshot":self.snapshot(),"verdict":"accept","findings":[]}),
        )
        .unwrap();
    }
}

#[test]
fn verification_contract_covers_acceptance_and_binds_actual_scopes_and_approved_checks() {
    let f = Fixture::new();
    f.project
        .write_state(
            "config.json",
            &json!({"checks":{"unit":{"argv":["/bin/sh","-c","exit 0"]}}}),
        )
        .unwrap();
    f.start("3");
    let tasks = json!([job("a", "src/a.py", &[]), job("b", "src/b.py", &[])]);
    let base = json!([
        obligation("boundary", &[0], &["a"], &["src/a.py"], &["unit"]),
        obligation("preserve", &[1], &["b"], &["src/b.py"], &["unit"])
    ]);
    let mut invalid = vec![json!([base[0].clone()]), json!(vec![base[0].clone(); 7])];
    for (field, value) in [
        ("id", json!("bad id")),
        ("id", json!("preserve")),
        ("acceptance", json!([])),
        ("acceptance", json!([2])),
        ("acceptance", json!([0, 0])),
        ("tasks", json!([])),
        ("tasks", json!(["unknown"])),
        ("tasks", json!(["a", "a"])),
        ("paths", json!([])),
        ("paths", json!(["src/b.py"])),
        ("paths", json!(["src"])),
        ("paths", json!(["../outside"])),
        ("paths", json!(["src/*.py"])),
        ("paths", json!(["src/a.py", "src/a.py"])),
        ("risk", json!(" ")),
        ("risk", json!("Я".repeat(769))),
        ("checks", json!([])),
        ("checks", json!(["unknown"])),
        ("checks", json!(["unit", "unit"])),
    ] {
        let mut plan = base.clone();
        plan[0][field] = value;
        invalid.push(plan);
    }
    for obligations in invalid {
        assert!(f
            .verification_plan(
                tasks.clone(),
                json!(["unit"]),
                json!(["Boundary is safe", "Sibling is preserved"]),
                obligations
            )
            .is_err());
        assert_eq!(f.run(&["status", "demo"]).unwrap()["phase"], "planning");
    }
    let accepted = f
        .verification_plan(
            tasks,
            json!(["unit"]),
            json!(["Boundary is safe", "Sibling is preserved"]),
            base.clone(),
        )
        .unwrap();
    assert_eq!(accepted["verification"], base);
    assert!(accepted["verification_report"].is_null());
}

#[test]
fn verification_results_are_complete_grounded_static_only_and_durable() {
    let f = Fixture::new();
    f.start("4");
    let contract = json!([obligation("boundary", &[0], &["a"], &["src/a.py"], &[])]);
    let tasks = json!([job("a", "src/a.py", &[])]);
    f.verification_plan(
        tasks.clone(),
        json!([]),
        json!(["Boundary is safe"]),
        contract.clone(),
    )
    .unwrap();
    f.apply(&[("a", "src/a.py")]).unwrap();
    f.accepted_review();
    let result = verification_result("boundary", "inspected", json!([]), "src/a.py");
    let base = json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"proofs":[],"verification":[result.clone()]});
    let mut missing = base.clone();
    missing.as_object_mut().unwrap().remove("verification");
    let mut invalid = vec![missing];
    for results in [json!([]), json!([result.clone(), result.clone()])] {
        let mut qa = base.clone();
        qa["verification"] = results;
        invalid.push(qa);
    }
    for (field, value) in [
        ("id", json!("other")),
        ("status", json!("passing")),
        ("status", json!("verified")),
        ("status", json!("gap")),
        ("evidence", json!(" ")),
        ("evidence", json!("Я".repeat(769))),
        ("locations", json!([])),
        ("locations", json!([{"path":"src/b.py","line":1}])),
        ("locations", json!([{"path":"src/a.py","line":0}])),
        ("locations", json!([{"path":"src/a.py","line":999}])),
        ("locations", json!([{"path":"src/missing.py","line":1}])),
        ("locations", json!([{"path":"../outside","line":1}])),
        ("proofs", json!(["fabricated"])),
    ] {
        let mut qa = base.clone();
        qa["verification"][0][field] = value;
        invalid.push(qa);
    }
    for qa in invalid {
        assert!(f.report("qa", qa).is_err());
        let status = f.run(&["status", "demo"]).unwrap();
        assert_eq!(status["phase"], "qa");
        assert!(status["verification_report"].is_null());
    }
    let done = f.report("qa", base).unwrap();
    assert_eq!(done["phase"], "done");
    assert_eq!(done["verification_execution"], "static_only");
    assert_eq!(
        done["verification_report"]["results"][0]["status"],
        "inspected"
    );
    assert_eq!(
        done["verification_report"]["results"][0]["proofs"],
        json!([])
    );
    assert_eq!(done["verification_report_fresh"], true);
    let reopened = Project::open(&f.root).unwrap();
    let restored: Value =
        serde_json::from_str(&workflow::run(&reopened, &["status".into(), "demo".into()]).unwrap())
            .unwrap();
    assert_eq!(restored["verification_report"], done["verification_report"]);
    fs::write(f.root.join("src/b.py"), "b=9\n").unwrap();
    assert_eq!(
        f.run(&["status", "demo"]).unwrap()["verification_report_fresh"],
        false
    );
}

#[test]
fn obligation_only_gap_repairs_its_task_and_consumers_preserving_independent_work() {
    let f = Fixture::new();
    f.start("4");
    let tasks = json!([
        job("a", "src/a.py", &[]),
        job("b", "src/b.py", &[]),
        job("consumer", "src/a.py", &["a"])
    ]);
    let obligations = json!([
        obligation("boundary", &[0], &["a"], &["src/a.py"], &[]),
        obligation("preserve", &[1], &["b"], &["src/b.py"], &[])
    ]);
    f.verification_plan(
        tasks,
        json!([]),
        json!(["A rejects atomically", "B keeps behavior"]),
        obligations,
    )
    .unwrap();
    f.apply(&[("a", "src/a.py"), ("b", "src/b.py")]).unwrap();
    f.apply(&[("consumer", "src/a.py")]).unwrap();
    f.accepted_review();
    let sibling = fs::read(f.root.join("src/b.py")).unwrap();
    let sibling_generation =
        f.run(&["status", "demo"]).unwrap()["task_states"]["b"]["generation"].clone();
    let mut gap = verification_result("boundary", "gap", json!([]), "src/a.py");
    gap["locations"] = json!([]);
    gap["evidence"] = json!("The newly required duplicate rejection regression is missing; current source inspection does not establish this acceptance condition.");
    let report = json!({"snapshot":f.snapshot(),"verdict":"changes","findings":[],"tasks":[],"proofs":[],
        "verification":[gap,verification_result("preserve","inspected",json!([]),"src/b.py")]});
    let repaired = f.report("qa", report).unwrap();
    assert_eq!(repaired["phase"], "developing");
    for id in ["a", "consumer"] {
        assert_eq!(repaired["task_states"][id]["status"], "pending");
    }
    assert_eq!(repaired["task_states"]["b"]["status"], "applied");
    assert_eq!(
        repaired["task_states"]["b"]["generation"],
        sibling_generation
    );
    assert_eq!(fs::read(f.root.join("src/b.py")).unwrap(), sibling);
    assert_eq!(repaired["verification_report_fresh"], true);
    assert!(repaired["findings"][0]
        .as_str()
        .unwrap()
        .starts_with("Verification boundary:"));
    f.apply(&[("a", "src/a.py")]).unwrap();
    let later = f.run(&["status", "demo"]).unwrap();
    assert_eq!(later["verification_report_fresh"], false);
    assert_eq!(
        later["verification_report"],
        repaired["verification_report"]
    );
}

#[test]
fn obligation_evidence_requires_actual_current_passing_named_proofs_and_all_global_checks() {
    let f = Fixture::new();
    f.project
        .write_state(
            "config.json",
            &json!({"checks":{
        "unit":{"argv":["/bin/sh","-c","test ! -f .ai-env/unit-fail"]},
        "integration":{"argv":["/bin/sh","-c","exit 0"]}}}),
        )
        .unwrap();
    f.start("3");
    f.verification_plan(
        json!([job("a", "src/a.py", &[])]),
        json!(["unit", "integration"]),
        json!(["Boundary is safe"]),
        json!([obligation(
            "boundary",
            &[0],
            &["a"],
            &["src/a.py"],
            &["unit"]
        )]),
    )
    .unwrap();
    let run = |check: &str| -> Value {
        serde_json::from_str(&verify::run(&f.project, &[check.into()]).unwrap()).unwrap()
    };
    let old = run("unit");
    f.apply(&[("a", "src/a.py")]).unwrap();
    f.accepted_review();
    let unit = run("unit");
    let integration = run("integration");
    fs::write(f.root.join(".ai-env/unit-fail"), "fail").unwrap();
    let failed = run("unit");
    fs::remove_file(f.root.join(".ai-env/unit-fail")).unwrap();
    assert_eq!(failed["passed"], false);
    let valid = verification_result("boundary", "verified", json!([unit["id"]]), "src/a.py");
    let base = json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"proofs":[unit["id"],integration["id"]],"verification":[valid.clone()]});
    for proof in [&old["id"], &failed["id"], &json!("fabricated")] {
        let mut qa = base.clone();
        qa["proofs"] = json!([proof, integration["id"]]);
        qa["verification"][0]["proofs"] = json!([proof]);
        assert!(f.report("qa", qa).is_err());
    }
    for proofs in [
        json!([]),
        json!([integration["id"]]),
        json!([unit["id"], unit["id"]]),
    ] {
        let mut qa = base.clone();
        qa["verification"][0]["proofs"] = proofs;
        assert!(f.report("qa", qa).is_err());
    }
    let mut inspected = base.clone();
    inspected["verification"][0]["status"] = json!("inspected");
    inspected["verification"][0]["proofs"] = json!([]);
    assert!(f.report("qa", inspected).is_err());
    let mut missing_global = base.clone();
    missing_global["proofs"] = json!([unit["id"]]);
    assert!(f.report("qa", missing_global).is_err());
    let unit_id = unit["id"].as_str().unwrap();
    for field in ["config_hash", "argv", "snapshot_after", "capture_complete"] {
        let mut forged = unit.clone();
        forged[field] = match field {
            "argv" => json!(["/bin/false"]),
            "capture_complete" => json!(false),
            _ => json!("wrong"),
        };
        f.project
            .write_state(&format!("proofs/{unit_id}.json"), &forged)
            .unwrap();
        assert!(f.report("qa", base.clone()).is_err());
    }
    f.project
        .write_state(&format!("proofs/{unit_id}.json"), &unit)
        .unwrap();
    let done = f.report("qa", base).unwrap();
    assert_eq!(done["phase"], "done");
    assert_eq!(done["verification_execution"], "approved_checks");
    assert_eq!(done["verification_report"]["results"][0], valid);
}

#[test]
fn a_passing_command_does_not_override_a_qa_semantic_coverage_gap() {
    let f = Fixture::new();
    f.project
        .write_state(
            "config.json",
            &json!({"checks":{"unit":{"argv":["/bin/sh","-c","exit 0"]}}}),
        )
        .unwrap();
    f.start("3");
    f.verification_plan(
        json!([job("a", "src/a.py", &[])]),
        json!(["unit"]),
        json!(["Boundary is safe"]),
        json!([obligation(
            "boundary",
            &[0],
            &["a"],
            &["src/a.py"],
            &["unit"]
        )]),
    )
    .unwrap();
    f.apply(&[("a", "src/a.py")]).unwrap();
    f.accepted_review();
    let passed: Value =
        serde_json::from_str(&verify::run(&f.project, &["unit".into()]).unwrap()).unwrap();
    assert_eq!(passed["passed"], true);
    let mut gap = verification_result("boundary", "gap", json!([passed["id"]]), "src/a.py");
    gap["evidence"] = json!("The approved command passed but did not exercise rejection without mutation; add a discoverable boundary regression before acceptance.");
    let repaired = f.report("qa", json!({"snapshot":f.snapshot(),"verdict":"changes","findings":[],"tasks":[],"proofs":[passed["id"]],"verification":[gap]})).unwrap();
    assert_eq!(repaired["phase"], "developing");
    assert_eq!(repaired["task_states"]["a"]["status"], "pending");
    assert_eq!(
        repaired["verification_report"]["results"][0]["status"],
        "gap"
    );
}

#[test]
fn revised_verification_contract_stales_evidence_without_replaying_unaffected_implementation() {
    let f = Fixture::new();
    f.start("5");
    let tasks = json!([job("a", "src/a.py", &[]), job("b", "src/b.py", &[])]);
    let contract = json!([
        obligation("boundary", &[0], &["a"], &["src/a.py"], &[]),
        obligation("sibling", &[0], &["b"], &["src/b.py"], &[])
    ]);
    f.verification_plan(
        tasks.clone(),
        json!([]),
        json!(["Boundary is safe"]),
        contract.clone(),
    )
    .unwrap();
    f.apply(&[("a", "src/a.py"), ("b", "src/b.py")]).unwrap();
    f.accepted_review();
    f.report(
        "qa",
        json!({"snapshot":f.snapshot(),"verdict":"changes","findings":[],"proofs":[],
        "verification":[verification_result("boundary","inspected",json!([]),"src/a.py"),
            verification_result("sibling","gap",json!([]),"src/b.py")]}),
    )
    .unwrap();
    let source = f.snapshot();
    f.run(&["replan", "demo"]).unwrap();
    let unchanged = f
        .verification_plan(
            tasks.clone(),
            json!([]),
            json!(["Boundary is safe"]),
            contract.clone(),
        )
        .unwrap();
    assert_eq!(unchanged["verification_report_fresh"], true);
    assert_eq!(unchanged["task_states"]["a"]["status"], "applied");
    f.run(&["replan", "demo"]).unwrap();
    let mut revised = contract;
    revised[0]["risk"] = json!("New boundary evidence must include the duplicate rejection path.");
    let changed = f
        .verification_plan(tasks, json!([]), json!(["Boundary is safe"]), revised)
        .unwrap();
    assert_eq!(changed["verification_report_fresh"], false);
    assert!(!changed["verification_report"].is_null());
    assert_eq!(changed["task_states"]["a"]["status"], "applied");
    assert_eq!(f.snapshot(), source);
}

#[test]
fn duplicate_obligation_results_with_matching_count_cannot_hide_a_missing_result() {
    let f = Fixture::new();
    f.start("3");
    f.verification_plan(
        json!([job("a", "src/a.py", &[]), job("b", "src/b.py", &[])]),
        json!([]),
        json!(["Both invariants hold"]),
        json!([
            obligation("first", &[0], &["a"], &["src/a.py"], &[]),
            obligation("second", &[0], &["b"], &["src/b.py"], &[])
        ]),
    )
    .unwrap();
    f.apply(&[("a", "src/a.py"), ("b", "src/b.py")]).unwrap();
    f.accepted_review();
    let first = verification_result("first", "inspected", json!([]), "src/a.py");
    assert!(f
        .report(
            "qa",
            json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"proofs":[],
        "verification":[first.clone(),first]})
        )
        .is_err());
    assert_eq!(f.run(&["status", "demo"]).unwrap()["phase"], "qa");
}

#[test]
fn verification_can_inspect_declared_read_context_but_cannot_escape_obligation_paths() {
    let f = Fixture::new();
    f.start("3");
    let mut task = job("a", "src/a.py", &[]);
    task["context"] = json!(["src/b.py"]);
    let tasks = json!([task]);
    f.verification_plan(
        tasks,
        json!([]),
        json!(["The reused policy is retained"]),
        json!([obligation("policy", &[0], &["a"], &["src/b.py"], &[])]),
    )
    .unwrap();
    f.apply(&[("a", "src/a.py")]).unwrap();
    f.accepted_review();
    let mut qa = json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"proofs":[],
        "verification":[verification_result("policy","inspected",json!([]),"src/a.py")]});
    assert!(
        f.report("qa", qa.clone()).is_err(),
        "A task-owned file outside this obligation is not grounding for it"
    );
    qa["verification"][0]["locations"][0]["path"] = json!("src/b.py");
    assert_eq!(f.report("qa", qa).unwrap()["phase"], "done");
}

#[test]
fn verification_gaps_merge_legacy_and_architecture_targets_without_narrowing_legacy_all() {
    for mode in ["legacy-target", "legacy-all", "architecture"] {
        let f = Fixture::new();
        fs::write(f.root.join("src/c.py"), "c=1\n").unwrap();
        f.start("3");
        f.report("plan", json!({"snapshot":f.snapshot(),"tasks":[job("a","src/a.py",&[]),job("b","src/b.py",&[]),job("c","src/c.py",&[])],
            "acceptance":["Boundary is safe"],"checks":[],"architecture":architecture_contract(),
            "verification":[obligation("boundary", &[0], &["a"], &["src/a.py"], &[])]})).unwrap();
        f.apply(&[("a", "src/a.py"), ("b", "src/b.py")]).unwrap();
        f.apply(&[("c", "src/c.py")]).unwrap();
        f.report("review", json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"architecture":architecture_assessment(json!([]))})).unwrap();
        let mut qa = json!({"snapshot":f.snapshot(),"verdict":"changes","findings":[],"tasks":[],"proofs":[],
            "verification":[verification_result("boundary","gap",json!([]),"src/a.py")]});
        match mode {
            "legacy-target" => {
                qa["findings"] = json!(["Task B misses the requested integration"]);
                qa["tasks"] = json!(["b"]);
            }
            "legacy-all" => {
                qa["findings"] = json!(["The integration failure cannot be localized to one task"]);
            }
            _ => {
                qa["architecture"] = architecture_assessment(json!([
                    architecture_issue("b", "src/b.py", "blocking"),
                    architecture_issue("c", "src/c.py", "advisory")
                ]));
            }
        }
        let repaired = f.report("qa", qa).unwrap();
        assert_eq!(repaired["task_states"]["a"]["status"], "pending", "{mode}");
        assert_eq!(repaired["task_states"]["b"]["status"], "pending", "{mode}");
        assert_eq!(
            repaired["task_states"]["c"]["status"],
            if mode == "legacy-all" {
                "pending"
            } else {
                "applied"
            },
            "{mode}"
        );
    }
}

#[test]
fn unchanged_obligation_gap_cannot_restart_repairs_by_rephrasing_evidence() {
    let f = Fixture::new();
    f.start("5");
    f.verification_plan(
        json!([job("a", "src/a.py", &[])]),
        json!([]),
        json!(["Boundary is safe"]),
        json!([obligation("boundary", &[0], &["a"], &["src/a.py"], &[])]),
    )
    .unwrap();
    f.apply(&[("a", "src/a.py")]).unwrap();
    let source = f.snapshot();
    for attempt in 0..2 {
        f.accepted_review();
        let mut gap = verification_result("boundary", "gap", json!([]), "src/a.py");
        gap["evidence"] = json!(format!(
            "Boundary regression remains missing; independent wording for attempt {attempt}."
        ));
        let state = f.report("qa", json!({"snapshot":source,"verdict":"changes","findings":[],"tasks":[],"proofs":[],"verification":[gap]})).unwrap();
        assert_eq!(
            state["phase"],
            if attempt == 0 {
                "developing"
            } else {
                "blocked"
            }
        );
        assert_eq!(f.snapshot(), source);
        if attempt == 0 {
            f.report("apply", json!({"snapshot":source,"jobs":[{"id":"a","summary":"Reinspected existing code without a source change","files":[]}]})).unwrap();
        } else {
            assert!(state["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|finding| finding.as_str().unwrap().contains("No progress")));
        }
    }
}

#[test]
fn obligation_gap_identity_ignores_variable_check_output_but_retains_proof_diagnostics() {
    let f = Fixture::new();
    f.project.write_state("config.json", &json!({"checks":{"unit":{"argv":["/bin/sh","-c",
        "if test -f .ai-env/seen-failure; then printf '1 failed in 0.99s\\n'; else touch .ai-env/seen-failure; printf '1 failed in 0.01s\\n'; fi; exit 1"]}}})).unwrap();
    f.start("5");
    f.verification_plan(
        json!([job("a", "src/a.py", &[])]),
        json!(["unit"]),
        json!(["Boundary is safe"]),
        json!([obligation(
            "boundary",
            &[0],
            &["a"],
            &["src/a.py"],
            &["unit"]
        )]),
    )
    .unwrap();
    f.apply(&[("a", "src/a.py")]).unwrap();
    let source = f.snapshot();
    let mut previous_output = None;
    for attempt in 0..2 {
        f.accepted_review();
        let proof: Value =
            serde_json::from_str(&verify::run(&f.project, &["unit".into()]).unwrap()).unwrap();
        assert_eq!(proof["passed"], false);
        if let Some(previous) = previous_output {
            assert_ne!(proof["stdout"], previous);
        }
        previous_output = Some(proof["stdout"].clone());
        let mut gap = verification_result("boundary", "gap", json!([proof["id"]]), "src/a.py");
        gap["evidence"] = json!(format!(
            "The boundary regression still fails; assessment wording {attempt}."
        ));
        let state = f
            .report(
                "qa",
                json!({"snapshot":source,"verdict":"changes","findings":[],"tasks":[],
            "proofs":[proof["id"]],"verification":[gap]}),
            )
            .unwrap();
        assert_eq!(
            state["phase"],
            if attempt == 0 {
                "developing"
            } else {
                "blocked"
            }
        );
        assert_eq!(state["failed_proofs"], json!([proof["id"]]));
        let saved = verify::load(&f.project, proof["id"].as_str().unwrap()).unwrap();
        assert_eq!(saved.stdout, proof["stdout"].as_str().unwrap());
        assert!(saved
            .stdout
            .contains(if attempt == 0 { "0.01s" } else { "0.99s" }));
        assert_eq!(f.snapshot(), source);
        if attempt == 0 {
            f.report("apply", json!({"snapshot":source,"jobs":[{"id":"a","summary":"Reinspected without changing source","files":[]}]})).unwrap();
        }
    }
}

#[test]
fn disjoint_ready_jobs_parallelize_and_dependencies_wait_for_merged_batch() {
    let f = Fixture::new();
    f.start("2");
    f.plan(
        json!([
            job("a", "src/a.py", &[]),
            job("b", "src/b.py", &[]),
            job("c", "src", &["a", "b"])
        ]),
        json!([]),
    )
    .unwrap();
    let ready = f.run(&["ready", "demo"]).unwrap();
    assert_eq!(ready["jobs"].as_array().unwrap().len(), 2);
    assert!(f.apply(&[("c", "src/a.py")]).is_err());
    f.apply(&[("a", "src/a.py"), ("b", "src/b.py")]).unwrap();
    let ready = f.run(&["ready", "demo"]).unwrap();
    assert_eq!(ready["jobs"][0]["id"], "c");
    f.apply(&[("c", "src/a.py")]).unwrap();
    assert_eq!(f.run(&["status", "demo"]).unwrap()["phase"], "reviewing");
}
#[test]
fn overlapping_jobs_serialize_and_duplicate_files_never_write() {
    let f = Fixture::new();
    f.start("2");
    f.plan(
        json!([job("all", "src", &[]), job("one", "src/a.py", &[])]),
        json!([]),
    )
    .unwrap();
    assert_eq!(
        f.run(&["ready", "demo"]).unwrap()["jobs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(f
        .apply(&[("all", "src/a.py"), ("one", "src/a.py")])
        .is_err());
    assert_eq!(
        fs::read_to_string(f.root.join("src/a.py")).unwrap(),
        "a=1\n"
    );
    f.apply(&[("all", "src/a.py")]).unwrap();
    assert_eq!(f.run(&["ready", "demo"]).unwrap()["jobs"][0]["id"], "one");
}
#[test]
fn malformed_plans_and_out_of_scope_edits_do_not_advance() {
    let f = Fixture::new();
    f.start("2");
    assert!(f
        .plan(
            json!([job("a", "src", &["b"]), job("b", "src", &["a"])]),
            json!([])
        )
        .is_err());
    assert!(f
        .plan(json!([job("a", "../elsewhere", &[])]), json!([]))
        .is_err());
    assert!(f
        .plan(json!([job("a", "src", &["missing"])]), json!([]))
        .is_err());
    f.plan(json!([job("a", "src/a.py", &[])]), json!([]))
        .unwrap();
    assert!(f.apply(&[("a", "src/b.py")]).is_err());
    assert_eq!(f.run(&["status", "demo"]).unwrap()["phase"], "developing");
}
#[test]
fn source_edits_invalidate_gates_and_replan_is_explicit_and_bounded() {
    let f = Fixture::new();
    f.start("2");
    f.plan(json!([job("a", "src", &[])]), json!([])).unwrap();
    fs::write(f.root.join("src/a.py"), "external=2\n").unwrap();
    assert_eq!(f.run(&["status", "demo"]).unwrap()["stale"], true);
    assert!(f.run(&["ready", "demo"]).is_err());
    assert!(f.apply(&[("a", "src/a.py")]).is_err());
    let state = f.run(&["replan", "demo"]).unwrap();
    assert_eq!(state["phase"], "planning");
    assert_eq!(state["round"], 2);
    assert_eq!(f.run(&["replan", "demo"]).unwrap()["phase"], "blocked");
}
#[test]
fn review_and_qa_are_ordered_and_rejection_budget_cannot_loop_forever() {
    let f = Fixture::new();
    f.start("1");
    let acceptance = json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"proofs":[]});
    assert!(f.report("qa", acceptance).is_err());
    f.plan(json!([job("a", "src", &[])]), json!([])).unwrap();
    f.apply(&[("a", "src/a.py")]).unwrap();
    let rejected = f
        .report(
            "review",
            json!({"snapshot":f.snapshot(),"verdict":"changes","findings":["Missing edge case"]}),
        )
        .unwrap();
    assert_eq!(rejected["phase"], "blocked");
    assert!(f.run(&["ready", "demo"]).is_err());
}
#[test]
fn qa_needs_actual_fresh_proofs_for_all_approved_checks_and_never_runs_deploy() {
    let f = Fixture::new();
    f.project.write_state("config.json",&json!({"checks":{"unit":{"argv":["/bin/sh","-c","exit 0"]},"integration":{"argv":["/bin/sh","-c","exit 0"]},"deploy":{"argv":["/bin/sh","-c","exit 99"]}}})).unwrap();
    f.start("2");
    assert!(f
        .plan(json!([job("a", "src", &[])]), json!(["unit"]))
        .is_err());
    assert!(f
        .plan(
            json!([job("a", "src", &[])]),
            json!(["unit", "integration", "deploy"])
        )
        .is_err());
    f.plan(
        json!([job("a", "src", &[])]),
        json!(["unit", "integration"]),
    )
    .unwrap();
    // A passing proof of the pre-change source is not sufficient after apply.
    let old: Value =
        serde_json::from_str(&verify::run(&f.project, &["unit".into()]).unwrap()).unwrap();
    f.apply(&[("a", "src/a.py")]).unwrap();
    f.report(
        "review",
        json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[]}),
    )
    .unwrap();
    assert!(f
        .report(
            "qa",
            json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"proofs":[old["id"]]})
        )
        .is_err());
    assert!(f.report("qa",json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"proofs":["fabricated"]})).is_err());
    let first: Value =
        serde_json::from_str(&verify::run(&f.project, &["unit".into()]).unwrap()).unwrap();
    assert!(f
        .report(
            "qa",
            json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"proofs":[first["id"]]})
        )
        .is_err());
    let second: Value =
        serde_json::from_str(&verify::run(&f.project, &["integration".into()]).unwrap()).unwrap();
    let done=f.report("qa",json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"proofs":[first["id"],second["id"]]})).unwrap();
    assert_eq!(done["phase"], "done");
    fs::write(f.root.join("src/b.py"), "changed=1\n").unwrap();
    assert_eq!(f.run(&["status", "demo"]).unwrap()["stale"], true);
}
#[test]
fn no_tests_are_reported_as_none_instead_of_passing_and_explicit_check_is_honored() {
    let f = Fixture::new();
    f.start("2");
    f.plan(json!([job("a", "src", &[])]), json!([])).unwrap();
    f.apply(&[("a", "src/a.py")]).unwrap();
    f.report(
        "review",
        json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[]}),
    )
    .unwrap();
    let done = f
        .report(
            "qa",
            json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"proofs":[]}),
        )
        .unwrap();
    assert_eq!(done["automated_checks"], "none_configured");
    assert_eq!(done["phase"], "done");
    let other = Fixture::new();
    other
        .project
        .write_state(
            "config.json",
            &json!({"checks":{"custom":{"argv":["/bin/true"]}}}),
        )
        .unwrap();
    other
        .run(&[
            "start",
            "demo",
            "--request",
            other.root.join("request.txt").to_str().unwrap(),
            "--check",
            "custom",
        ])
        .unwrap();
    assert!(other
        .plan(json!([job("a", "src", &[])]), json!([]))
        .is_err());
    other
        .plan(json!([job("a", "src", &[])]), json!(["custom"]))
        .unwrap();
}

#[test]
fn already_satisfied_jobs_complete_without_gratuitous_source_edits() {
    let f = Fixture::new();
    f.start("2");
    f.plan(json!([job("a", "src/a.py", &[])]), json!([]))
        .unwrap();
    let snapshot = f.snapshot();
    let state=f.report("apply",json!({"snapshot":snapshot,"jobs":[{"id":"a","summary":"Existing implementation already satisfies this requirement; verified source contract", "files":[]}]})).unwrap();
    assert_eq!(state["phase"], "reviewing");
    assert_eq!(state["snapshot"], snapshot);
    assert_eq!(f.snapshot(), snapshot);
    assert_eq!(
        fs::read_to_string(f.root.join("src/a.py")).unwrap(),
        "a=1\n"
    );
}

#[test]
fn workflow_apply_preserves_existing_task_claims_and_read_context_is_not_a_write_scope() {
    let f = Fixture::new();
    f.start("2");
    let mut first = job("a", "src/a.py", &["b"]);
    first["context"] = json!(["src/b.py"]);
    f.plan(json!([first, job("b", "src/b.py", &[])]), json!([]))
        .unwrap();
    assert_eq!(f.run(&["ready", "demo"]).unwrap()["jobs"][0]["id"], "b");
    for args in [
        vec![
            "add",
            "other",
            "--title",
            "Other agent work",
            "--paths",
            "src/a.py",
        ],
        vec!["claim", "other", "--owner", "another-agent"],
    ] {
        aienv::tasks::run(
            &f.project,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap();
    }
    f.apply(&[("b", "src/b.py")]).unwrap();
    assert!(f.apply(&[("a", "src/b.py")]).is_err());
    assert!(f.apply(&[("a", "src/a.py")]).is_err());
    assert_eq!(
        fs::read_to_string(f.root.join("src/a.py")).unwrap(),
        "a=1\n"
    );
    assert!(f.run(&["status", "demo"]).unwrap()["completed"]
        .as_object()
        .unwrap()
        .is_empty());
}

#[test]
fn preview_is_read_only_and_validates_target_scope_index_and_expected_hash() {
    let f = Fixture::new();
    f.start("2");
    f.plan(json!([job("a", "src", &[])]), json!([])).unwrap();
    let snapshot = f.project.snapshot().unwrap();
    let valid = json!({"snapshot":snapshot.hash,"jobs":[{"id":"a","summary":"Scoped change", "files":[
        {"path":"src/a.py","expected_hash":snapshot.files["src/a.py"].hash,"source":"a=2\n"},
        {"path":"src/new.py","expected_hash":null,"source":"new=1\n"}
    ]}]});
    let preview = f.report("preview", valid.clone()).unwrap();
    assert_eq!(preview["originals"]["src/a.py"], "a=1\n");
    assert!(preview["originals"]["src/new.py"].is_null());
    assert_eq!(preview["originals"].as_object().unwrap().len(), 2);
    assert_eq!(f.snapshot(), snapshot.hash);
    assert!(!f.root.join("src/new.py").exists());
    assert!(f.run(&["status", "demo"]).unwrap()["completed"]
        .as_object()
        .unwrap()
        .is_empty());
    let mut bad = valid.clone();
    bad["jobs"][0]["files"][0]["expected_hash"] = json!("wrong");
    assert!(f.report("preview", bad).is_err());
    let mut bad = valid.clone();
    bad["jobs"][0]["files"][0]["path"] = json!("request.txt");
    assert!(f.report("preview", bad).is_err());
    fs::write(f.root.join("src/data.bin"), b"a\0b").unwrap();
    let bad = json!({"snapshot":f.snapshot(),"jobs":[{"id":"a","summary":"Not an indexed source", "files":[{"path":"src/data.bin","expected_hash":null,"source":"replacement"}]}]});
    let error = f.report("preview", bad).unwrap_err().to_string();
    assert!(error.contains("not indexed"), "{error}");
    assert_eq!(fs::read(f.root.join("src/data.bin")).unwrap(), b"a\0b");
}

#[test]
fn checks_discovery_is_read_only_and_uses_the_start_default_filter() {
    let f = Fixture::new();
    f.project
        .write_state(
            "config.json",
            &json!({"checks":{
                "deploy":{"argv":["/bin/false"]},"CUSTOM":{"argv":["/bin/true"]},
                "unit-fast":{"argv":["/bin/true"]},"Integration_Suite":{"argv":["/bin/true"]},
                "lint":{"argv":["/bin/true"]}
            }}),
        )
        .unwrap();
    let snapshot = f.snapshot();
    let checks = f.run(&["checks"]).unwrap();
    assert_eq!(
        checks["available"],
        json!(["CUSTOM", "Integration_Suite", "deploy", "lint", "unit-fast"])
    );
    assert_eq!(
        checks["defaults"],
        json!(["Integration_Suite", "lint", "unit-fast"])
    );
    assert_eq!(f.snapshot(), snapshot);
    assert!(!f.project.state.join("workflows").exists());
    assert!(f.run(&["checks", "extra"]).is_err());
    f.start("2");
    assert_eq!(
        f.run(&["status", "demo"]).unwrap()["required_checks"],
        checks["defaults"]
    );
}

impl Fixture {
    fn reserve(&self, task: &str) -> Value {
        self.report("attempt-start", json!({"task":task,"owner":"test-worker"}))
            .unwrap()["attempt"]
            .clone()
    }
    fn propose(&self, attempt: &Value, path: &str, source: &str) -> aienv::Result<Value> {
        let snapshot = self.project.snapshot().unwrap();
        self.report("attempt-propose", json!({"attempt":attempt["id"],"snapshot":attempt["snapshot"],"summary":"Scoped implementation", "edits":[{"path":path,"expected_hash":snapshot.files.get(path).map(|file| &file.hash),"source":source}]}))
    }
    fn apply_attempt(&self, attempt: &Value) -> aienv::Result<Value> {
        self.report("attempt-apply", json!({"attempts":[attempt["id"]]}))
    }
}

#[test]
fn durable_success_survives_sibling_failure_and_only_failed_task_retries() {
    let f = Fixture::new();
    f.start("3");
    f.plan(
        json!([
            job("a", "src/a.py", &[]),
            job("b", "src/b.py", &[]),
            job("combined", "src", &["a", "b"])
        ]),
        json!([]),
    )
    .unwrap();
    let a = f.reserve("a");
    let b = f.reserve("b");
    assert!(f.run(&["ready", "demo"]).unwrap()["jobs"]
        .as_array()
        .unwrap()
        .is_empty());
    f.propose(&a, "src/a.py", "a=2\n").unwrap();
    f.report(
        "attempt-fail",
        json!({"attempt":b["id"],"error":"Compiler rejected invalid expression"}),
    )
    .unwrap();
    let restored = Project::open(&f.root).unwrap();
    let state: Value =
        serde_json::from_str(&workflow::run(&restored, &["status".into(), "demo".into()]).unwrap())
            .unwrap();
    assert_eq!(state["task_states"]["a"]["status"], "proposed");
    let preview = f
        .report("attempt-preview", json!({"attempts":[a["id"]]}))
        .unwrap();
    assert_eq!(preview["originals"]["src/a.py"], "a=1\n");
    f.apply_attempt(&a).unwrap();
    let state = f.run(&["status", "demo"]).unwrap();
    assert_eq!(state["task_states"]["a"]["status"], "applied");
    assert!(state["completed"].as_object().unwrap().is_empty());
    assert_eq!(f.run(&["ready", "demo"]).unwrap()["jobs"][0]["id"], "b");
    let retry = f.reserve("b");
    assert_ne!(retry["id"], b["id"]);
    f.propose(&retry, "src/b.py", "b=2\n").unwrap();
    f.apply_attempt(&retry).unwrap();
    assert_eq!(
        f.run(&["ready", "demo"]).unwrap()["jobs"][0]["id"],
        "combined"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("src/a.py")).unwrap(),
        "a=2\n"
    );
    assert_eq!(
        f.run(&["status", "demo"]).unwrap()["attempts"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn final_sibling_failure_preserves_late_valid_proposal_without_unblocking_or_applying() {
    for failure_first in [true, false] {
        let f = Fixture::new();
        f.start("1");
        f.plan(
            json!([job("a", "src/a.py", &[]), job("b", "src/b.py", &[])]),
            json!([]),
        )
        .unwrap();
        let a = f.reserve("a");
        let b = f.reserve("b");
        let original = f.project.snapshot().unwrap();
        if !failure_first {
            f.propose(&a, "src/a.py", "a=2\n").unwrap();
        }
        let blocked = f
            .report(
                "attempt-fail",
                json!({"attempt":b["id"],"error":"Final allowed sibling attempt failed"}),
            )
            .unwrap();
        assert_eq!(blocked["phase"], "blocked");
        if failure_first {
            // Being blocked does not relax the running worker's CAS or scope.
            assert!(f.report("attempt-propose", json!({"attempt":a["id"],"snapshot":a["snapshot"],"summary":"Stale edit","edits":[{"path":"src/a.py","expected_hash":"stale","source":"a=2\n"}]})).is_err());
            assert!(f.propose(&a, "src/b.py", "b=2\n").is_err());
            f.propose(&a, "src/a.py", "a=2\n").unwrap();
        }
        let status = f.run(&["status", "demo"]).unwrap();
        assert_eq!(status["phase"], "blocked");
        assert_eq!(status["attempts"][0]["status"], "proposed");
        assert_eq!(status["attempts"][1]["status"], "failed");
        assert_eq!(status["task_states"]["a"]["status"], "proposed");
        assert_eq!(f.snapshot(), original.hash);
        assert!(f.apply_attempt(&a).is_err());
        assert!(f
            .report("attempt-preview", json!({"attempts":[a["id"]]}))
            .is_err());
        assert!(f.run(&["ready", "demo"]).is_err());
        assert!(f
            .report("attempt-start", json!({"task":"b","owner":"new-worker"}))
            .is_err());
        let detail = f
            .run(&[
                "attempt-status",
                "demo",
                "--attempt",
                a["id"].as_str().unwrap(),
            ])
            .unwrap();
        assert_eq!(detail["proposal"]["files"][0]["source"], "a=2\n");
        assert_eq!(
            detail["proposal"]["files"][0]["expected_hash"],
            original.files["src/a.py"].hash
        );
        let reopened = Project::open(&f.root).unwrap();
        let restored: Value = serde_json::from_str(
            &workflow::run(&reopened, &["status".into(), "demo".into()]).unwrap(),
        )
        .unwrap();
        assert_eq!(restored["phase"], "blocked");
        assert_eq!(restored["attempts"][0]["status"], "proposed");
        assert_eq!(f.snapshot(), original.hash);
    }
}

#[test]
fn saved_parallel_proposals_rebase_only_over_disjoint_declared_inputs() {
    let f = Fixture::new();
    f.start("2");
    f.plan(
        json!([job("a", "src/a.py", &[]), job("b", "src/b.py", &[])]),
        json!([]),
    )
    .unwrap();
    let a = f.reserve("a");
    let b = f.reserve("b");
    f.propose(&a, "src/a.py", "a=2\n").unwrap();
    f.propose(&b, "src/b.py", "b=2\n").unwrap();
    f.apply_attempt(&a).unwrap();
    assert_ne!(f.snapshot(), b["snapshot"].as_str().unwrap());
    f.apply_attempt(&b).unwrap();
    assert_eq!(f.run(&["status", "demo"]).unwrap()["phase"], "reviewing");
}

#[test]
fn recovery_restores_saved_proposal_and_releases_only_interrupted_attempt() {
    let f = Fixture::new();
    f.start("3");
    f.plan(
        json!([job("a", "src/a.py", &[]), job("b", "src/b.py", &[])]),
        json!([]),
    )
    .unwrap();
    let a = f.reserve("a");
    let b = f.reserve("b");
    f.propose(&a, "src/a.py", "a=2\n").unwrap();
    // Crash after proposal sync but before the workflow queue transition.
    let mut raw: Value = f.project.read_state("workflows/demo.json").unwrap();
    raw["attempts"][0]["status"] = json!("running");
    raw["task_states"]["a"]["status"] = json!("running");
    f.project.write_state("workflows/demo.json", &raw).unwrap();
    let recovered = f.run(&["attempt-recover", "demo"]).unwrap();
    assert_eq!(recovered["attempts"][0]["status"], "proposed");
    assert_eq!(recovered["attempts"][1]["id"], b["id"]);
    assert_eq!(recovered["attempts"][1]["status"], "interrupted");
    f.apply_attempt(&a).unwrap();
    assert_eq!(f.run(&["ready", "demo"]).unwrap()["jobs"][0]["id"], "b");
}

#[test]
fn targeted_repair_preserves_unaffected_work_and_invalidates_dependents() {
    let f = Fixture::new();
    f.start("3");
    let tasks = json!([
        job("a", "src/a.py", &[]),
        job("b", "src/b.py", &[]),
        job("consumer", "src/a.py", &["a"])
    ]);
    f.plan(tasks.clone(), json!([])).unwrap();
    f.apply(&[("a", "src/a.py"), ("b", "src/b.py")]).unwrap();
    f.apply(&[("consumer", "src/a.py")]).unwrap();
    let state=f.report("review",json!({"snapshot":f.snapshot(),"verdict":"changes","findings":["A is missing an edge case"],"tasks":["a"]})).unwrap();
    assert_eq!(state["phase"], "developing");
    assert_eq!(state["task_states"]["b"]["status"], "applied");
    assert_eq!(state["task_states"]["a"]["status"], "pending");
    assert_eq!(state["task_states"]["consumer"]["status"], "pending");
    assert_eq!(state["tasks"].as_array().unwrap().len(), 3);
    let ready = f.run(&["ready", "demo"]).unwrap();
    assert_eq!(ready["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(ready["jobs"][0]["id"], "a");
}

#[test]
fn explicit_replan_preserves_unchanged_contract_and_scoped_result() {
    let f = Fixture::new();
    f.start("3");
    let tasks = json!([job("a", "src/a.py", &[]), job("b", "src/b.py", &[])]);
    f.plan(tasks.clone(), json!([])).unwrap();
    f.apply(&[("a", "src/a.py"), ("b", "src/b.py")]).unwrap();
    fs::write(f.root.join("src/b.py"), "external_b=3\n").unwrap();
    let state = f.run(&["replan", "demo"]).unwrap();
    assert_eq!(state["task_states"]["a"]["status"], "applied");
    assert_eq!(state["task_states"]["b"]["status"], "pending");
    f.plan(tasks, json!([])).unwrap();
    assert_eq!(f.run(&["ready", "demo"]).unwrap()["jobs"][0]["id"], "b");
    assert_eq!(
        f.run(&["status", "demo"]).unwrap()["summaries"]["a"],
        "Implemented and tested"
    );
}

#[test]
fn unchanged_repeated_failure_stops_even_with_remaining_retry_budget() {
    let f = Fixture::new();
    f.start("5");
    f.plan(json!([job("a", "src/a.py", &[])]), json!([]))
        .unwrap();
    for index in 0..2 {
        let attempt = f.reserve("a");
        let state = f
            .report(
                "attempt-fail",
                json!({"attempt":attempt["id"],"error":"Same syntax failure at line 2"}),
            )
            .unwrap();
        assert_eq!(
            state["phase"],
            if index == 0 { "developing" } else { "blocked" }
        );
    }
    let state = f.run(&["status", "demo"]).unwrap();
    assert!(state["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding.as_str().unwrap().contains("No progress")));
    assert!(f.run(&["ready", "demo"]).is_err());
}

#[test]
fn single_executor_has_distinct_applied_verified_and_completed_gates() {
    let f = Fixture::new();
    f.run(&[
        "start",
        "demo",
        "--request",
        f.root.join("request.txt").to_str().unwrap(),
        "--mode",
        "single",
    ])
    .unwrap();
    f.plan(json!([job("only", "src/a.py", &[])]), json!([]))
        .unwrap();
    let attempt = f.reserve("only");
    f.propose(&attempt, "src/a.py", "a=2\n").unwrap();
    let applied = f.apply_attempt(&attempt).unwrap();
    assert_eq!(applied["phase"], "qa");
    assert_eq!(applied["task_states"]["only"]["status"], "applied");
    let verified = f
        .report("verify", json!({"snapshot":f.snapshot(),"proofs":[]}))
        .unwrap();
    assert_eq!(verified["task_states"]["only"]["status"], "verified");
    assert_eq!(verified["phase"], "qa");
    assert!(f
        .report(
            "qa",
            json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"proofs":[]})
        )
        .is_err());
    let done=f.report("qa",json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"proofs":[],"assessment":["The requested assignment is implemented in the scoped source; no automated checks are configured"]})).unwrap();
    assert_eq!(done["phase"], "done");
    assert!(!done["assessment"].as_array().unwrap().is_empty());
    assert_eq!(done["task_states"]["only"]["status"], "completed");
    assert_eq!(done["completed"]["only"], f.snapshot());
}

#[test]
fn recovery_acknowledges_committed_intent_without_reapplying_and_blocks_ambiguity() {
    let f = Fixture::new();
    f.start("3");
    f.plan(json!([job("a", "src/a.py", &[])]), json!([]))
        .unwrap();
    let mut before: Value = f.project.read_state("workflows/demo.json").unwrap();
    let hash = f.snapshot();
    f.apply(&[("a", "src/a.py")]).unwrap();
    let after = f.snapshot();
    before["pending"] = json!({"before":hash,"after":after,"jobs":[{"id":"a","summary":"Recovered committed result","files":[]}],"attempts":[]});
    f.project
        .write_state("workflows/demo.json", &before)
        .unwrap();
    let recovered = f.run(&["attempt-recover", "demo"]).unwrap();
    assert_eq!(recovered["phase"], "reviewing");
    assert!(recovered["pending_apply"].is_null());
    assert_eq!(recovered["summaries"]["a"], "Recovered committed result");
    assert_eq!(
        fs::read_to_string(f.root.join("src/a.py")).unwrap(),
        "a=1\n# edited\n"
    );
    f.project
        .write_state("workflows/demo.json", &before)
        .unwrap();
    fs::write(f.root.join("src/b.py"), "ambiguous=3\n").unwrap();
    let blocked = f.run(&["attempt-recover", "demo"]).unwrap();
    assert_eq!(blocked["phase"], "blocked");
    assert!(!blocked["pending_apply"].is_null());
    assert_eq!(
        fs::read_to_string(f.root.join("src/b.py")).unwrap(),
        "ambiguous=3\n"
    );
}

#[test]
fn plan_requires_provider_dependencies_and_proposals_validate_destinations_early() {
    let f = Fixture::new();
    f.start("3");
    let mut reader = job("reader", "src/a.py", &[]);
    reader["context"] = json!(["src/b.py"]);
    assert!(f
        .plan(json!([reader, job("writer", "src/b.py", &[])]), json!([]))
        .is_err());
    f.plan(json!([job("a", "src", &[])]), json!([])).unwrap();
    let attempt = f.reserve("a");
    fs::write(f.root.join("src/data.bin"), b"a\0b").unwrap();
    assert!(f
        .propose(&attempt, "src/data.bin", "new data")
        .unwrap_err()
        .to_string()
        .contains("not indexed"));
    assert_eq!(
        f.run(&["status", "demo"]).unwrap()["attempts"][0]["status"],
        "running"
    );
    assert!(f
        .propose(&attempt, "src/.gitignore", "*.py\n")
        .unwrap_err()
        .to_string()
        .contains("index discovery"));
}

#[test]
fn changed_style_rules_invalidate_scope_result_and_workflow_list_is_compact() {
    let f = Fixture::new();
    fs::write(f.root.join(".editorconfig"), "[*]\nindent_size=2\n").unwrap();
    f.start("3");
    let tasks = json!([job("a", "src/a.py", &[]), job("b", "src/b.py", &[])]);
    f.plan(tasks.clone(), json!([])).unwrap();
    f.apply(&[("a", "src/a.py"), ("b", "src/b.py")]).unwrap();
    fs::write(f.root.join(".editorconfig"), "[*]\nindent_size=4\n").unwrap();
    let state = f.run(&["replan", "demo"]).unwrap();
    assert_eq!(state["task_states"]["a"]["status"], "pending");
    assert_eq!(state["task_states"]["b"]["status"], "pending");
    let listing = f.run(&["list"]).unwrap();
    assert_eq!(listing["workflows"][0]["id"], "demo");
    assert!(listing["workflows"][0].get("attempts").unwrap().is_number());
}

#[test]
fn verified_siblings_become_applied_when_combined_proof_gate_is_invalidated() {
    let f = Fixture::new();
    f.start("3");
    f.plan(
        json!([job("a", "src/a.py", &[]), job("b", "src/b.py", &[])]),
        json!([]),
    )
    .unwrap();
    f.apply(&[("a", "src/a.py"), ("b", "src/b.py")]).unwrap();
    f.report(
        "review",
        json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[]}),
    )
    .unwrap();
    f.report("verify", json!({"snapshot":f.snapshot(),"proofs":[]}))
        .unwrap();
    let state=f.report("qa",json!({"snapshot":f.snapshot(),"verdict":"changes","findings":["Only A misses requested behavior"],"tasks":["a"],"proofs":[]})).unwrap();
    assert_eq!(state["task_states"]["a"]["status"], "pending");
    assert_eq!(state["task_states"]["b"]["status"], "applied");
    assert!(state["proofs"].as_array().unwrap().is_empty());
}

#[test]
fn failed_real_check_identity_stops_unchanged_noop_repair_despite_new_proof_and_prose() {
    let f = Fixture::new();
    f.project.write_state("config.json",&json!({"checks":{"unit":{"argv":["/bin/sh","-c","printf 'same regression'; exit 1"]}}})).unwrap();
    f.start("5");
    f.plan(json!([job("a", "src/a.py", &[])]), json!(["unit"]))
        .unwrap();
    f.apply(&[("a", "src/a.py")]).unwrap();
    for index in 0..2 {
        f.report(
            "review",
            json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[]}),
        )
        .unwrap();
        let proof: Value =
            serde_json::from_str(&verify::run(&f.project, &["unit".into()]).unwrap()).unwrap();
        assert_eq!(proof["passed"], false);
        let state=f.report("qa",json!({"snapshot":f.snapshot(),"verdict":"changes","findings":[format!("Different assessment wording number {index}; report {}",proof["id"])],"tasks":["a"],"proofs":[proof["id"]]})).unwrap();
        assert_eq!(state["failed_proofs"][0], proof["id"]);
        if index == 0 {
            assert_eq!(state["phase"], "developing");
            f.report("apply",json!({"snapshot":f.snapshot(),"jobs":[{"id":"a","summary":"No code changes; incorrectly claimed fixed","files":[]}]})).unwrap();
        } else {
            assert_eq!(state["phase"], "blocked");
        }
    }
}

#[test]
fn unindexed_or_linked_new_destination_is_rejected_before_proposal_transition() {
    let f = Fixture::new();
    f.start("3");
    f.plan(json!([job("a", "src", &[])]), json!([])).unwrap();
    let attempt = f.reserve("a");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&f.root, f.root.join("src/linked")).unwrap();
        assert!(f
            .propose(&attempt, "src/linked/new.py", "danger=1\n")
            .is_err());
        assert!(!f.root.join("new.py").exists());
    }
    assert_eq!(
        f.run(&["status", "demo"]).unwrap()["task_states"]["a"]["status"],
        "running"
    );
}

#[test]
fn explicit_replan_keeps_durable_proposal_after_unrelated_external_change() {
    let f = Fixture::new();
    f.start("3");
    let tasks = json!([job("a", "src/a.py", &[])]);
    f.plan(tasks.clone(), json!([])).unwrap();
    let a = f.reserve("a");
    f.propose(&a, "src/a.py", "a=2\n").unwrap();
    fs::write(f.root.join("src/b.py"), "external_b=9\n").unwrap();
    let state = f.run(&["replan", "demo"]).unwrap();
    assert_eq!(state["task_states"]["a"]["status"], "proposed");
    f.plan(tasks, json!([])).unwrap();
    f.apply_attempt(&a).unwrap();
    assert_eq!(
        fs::read_to_string(f.root.join("src/a.py")).unwrap(),
        "a=2\n"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("src/b.py")).unwrap(),
        "external_b=9\n"
    );
    assert_eq!(
        f.run(&["status", "demo"]).unwrap()["attempts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn replan_compares_external_changes_against_last_authorized_merged_baseline() {
    let f = Fixture::new();
    f.start("3");
    let tasks = json!([
        job("provider", "src/a.py", &[]),
        job("consumer", "src/a.py", &["provider"])
    ]);
    f.plan(tasks.clone(), json!([])).unwrap();
    f.apply(&[("provider", "src/a.py")]).unwrap();
    f.apply(&[("consumer", "src/a.py")]).unwrap();
    fs::write(f.root.join("src/b.py"), "unrelated_external=9\n").unwrap();
    let state = f.run(&["replan", "demo"]).unwrap();
    assert_eq!(state["task_states"]["provider"]["status"], "applied");
    assert_eq!(state["task_states"]["consumer"]["status"], "applied");
    let state = f.plan(tasks, json!([])).unwrap();
    assert_eq!(state["phase"], "reviewing");
}

#[test]
fn changed_implicit_metadata_releases_stale_proposal_without_interrupting_other_sessions() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("config")).unwrap();
    fs::write(f.root.join("config/.editorconfig"), "[*]\nindent_size=2\n").unwrap();
    f.start("3");
    f.plan(
        json!([job("a", "src/a.py", &[]), job("config", "config", &[])]),
        json!([]),
    )
    .unwrap();
    let a = f.reserve("a");
    let config = f.reserve("config");
    f.propose(&a, "src/a.py", "a=2\n").unwrap();
    f.propose(&config, "config/.editorconfig", "[*]\nindent_size=4\n")
        .unwrap();
    f.apply_attempt(&config).unwrap();
    assert!(f.apply_attempt(&a).is_err());
    let state = f.run(&["attempt-refresh", "demo"]).unwrap();
    assert_eq!(state["attempts"][0]["status"], "superseded");
    assert_eq!(state["task_states"]["a"]["status"], "pending");
    assert_eq!(state["task_states"]["config"]["status"], "applied");
    let retry = f.reserve("a");
    let state = f.run(&["attempt-refresh", "demo"]).unwrap();
    assert_eq!(state["attempts"][2]["id"], retry["id"]);
    assert_eq!(state["attempts"][2]["status"], "running");
}

#[test]
fn explicitly_scoped_global_rules_writer_must_precede_all_consumers() {
    let f = Fixture::new();
    fs::write(f.root.join("AGENTS.md"), "Use existing conventions\n").unwrap();
    f.start("3");
    assert!(f
        .plan(
            json!([job("a", "src/a.py", &[]), job("rules", "AGENTS.md", &[])]),
            json!([])
        )
        .is_err());
    f.plan(
        json!([
            job("a", "src/a.py", &["rules"]),
            job("rules", "AGENTS.md", &[])
        ]),
        json!([]),
    )
    .unwrap();
    assert_eq!(f.run(&["ready", "demo"]).unwrap()["jobs"][0]["id"], "rules");
}

#[test]
fn grouped_replacements_persist_preview_and_commit_as_one_original_hash_transaction() {
    let f = Fixture::new();
    fs::write(f.root.join("src/a.py"), "a=1\r\nkeep=1\r\n").unwrap();
    f.start("2");
    f.plan(json!([job("a", "src", &[])]), json!([])).unwrap();
    let attempt = f.reserve("a");
    let snapshot = f.project.snapshot().unwrap();
    let proposal = json!({
        "attempt":attempt["id"],"snapshot":attempt["snapshot"],"summary":"Adapt two related declarations",
        "edits":[
            {"path":"src/a.py","expected_hash":snapshot.files["src/a.py"].hash,
             "replacements":[{"old":"a=1","new":"a=2"},{"old":"a=2","new":"a=3"},{"old":"keep=1","new":"keep=2"}]},
            {"path":"src/b.py","expected_hash":snapshot.files["src/b.py"].hash,"old":"b=1","new":"b=2"}
        ]
    });
    f.report("attempt-propose", proposal).unwrap();
    // A new Project instance reads the durable proposal rather than a live agent's context.
    let resumed = Project::open(&f.root).unwrap();
    let status: Value =
        serde_json::from_str(&workflow::run(&resumed, &["status".into(), "demo".into()]).unwrap())
            .unwrap();
    assert_eq!(status["attempts"][0]["status"], "proposed");
    let preview = f
        .report("attempt-preview", json!({"attempts":[attempt["id"]]}))
        .unwrap();
    assert_eq!(preview["originals"]["src/a.py"], "a=1\r\nkeep=1\r\n");
    assert_eq!(f.snapshot(), snapshot.hash);
    let applied = f.apply_attempt(&attempt).unwrap();
    assert_eq!(
        fs::read(f.root.join("src/a.py")).unwrap(),
        b"a=3\r\nkeep=2\r\n"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("src/b.py")).unwrap(),
        "b=2\n"
    );
    assert_eq!(applied["snapshot"], f.snapshot());
    assert_eq!(applied["attempts"][0]["status"], "applied");
    assert!(applied["pending_apply"].is_null());
}

#[test]
fn failed_late_replacement_never_writes_earlier_files_or_creates_apply_intent() {
    let f = Fixture::new();
    f.start("2");
    f.plan(json!([job("a", "src", &[])]), json!([])).unwrap();
    let snapshot = f.project.snapshot().unwrap();
    let batch = json!({"snapshot":snapshot.hash,"jobs":[{"id":"a","summary":"Scoped changes","files":[
        {"path":"src/b.py","expected_hash":snapshot.files["src/b.py"].hash,"source":"b=2\n"},
        {"path":"src/a.py","expected_hash":snapshot.files["src/a.py"].hash,
         "replacements":[{"old":"a=1","new":"a=2"},{"old":"missing","new":"later"}]}
    ]}]});
    for action in ["preview", "apply"] {
        let error = f.report(action, batch.clone()).unwrap_err().to_string();
        assert!(error.contains("exactly once"), "{error}");
        assert_eq!(f.snapshot(), snapshot.hash);
        assert_eq!(
            fs::read_to_string(f.root.join("src/a.py")).unwrap(),
            "a=1\n"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("src/b.py")).unwrap(),
            "b=1\n"
        );
        let state = f.run(&["status", "demo"]).unwrap();
        assert!(state["pending_apply"].is_null());
        assert_eq!(state["task_states"]["a"]["status"], "pending");
    }
}

#[test]
fn grouped_replacements_reject_ambiguous_matches_including_new_and_overlapping_occurrences() {
    let f = Fixture::new();
    fs::write(f.root.join("src/a.py"), "aaa\nx=1\n").unwrap();
    f.start("2");
    f.plan(json!([job("a", "src/a.py", &[])]), json!([]))
        .unwrap();
    let snapshot = f.project.snapshot().unwrap();
    for replacements in [
        json!([{"old":"aa","new":"b"}]),
        json!([{"old":"","new":"b"}]),
        json!([{"old":"x=1","new":"aaa"},{"old":"aaa","new":"b"}]),
    ] {
        let batch = json!({"snapshot":snapshot.hash,"jobs":[{"id":"a","summary":"Replace symbols","files":[
            {"path":"src/a.py","expected_hash":snapshot.files["src/a.py"].hash,"replacements":replacements}
        ]}]});
        assert!(f.report("preview", batch.clone()).is_err());
        assert!(f.report("apply", batch).is_err());
        assert_eq!(f.snapshot(), snapshot.hash);
    }
}

#[test]
fn replacement_forms_hashes_scopes_and_bounds_cannot_bypass_the_transaction() {
    let f = Fixture::new();
    f.start("2");
    f.plan(json!([job("a", "src/a.py", &[])]), json!([]))
        .unwrap();
    let snapshot = f.project.snapshot().unwrap();
    let base = json!({"path":"src/a.py","expected_hash":snapshot.files["src/a.py"].hash,
        "replacements":[{"old":"a=1","new":"a=2"}]});
    let mut invalid = Vec::new();
    let mut edit = base.clone();
    edit["expected_hash"] = json!("wrong");
    invalid.push(vec![edit]);
    let mut edit = base.clone();
    edit["path"] = json!("src/b.py");
    edit["expected_hash"] = json!(snapshot.files["src/b.py"].hash);
    invalid.push(vec![edit]);
    let mut edit = base.clone();
    edit["source"] = json!("whole-file");
    invalid.push(vec![edit]);
    let mut edit = base.clone();
    edit["old"] = json!("a=1");
    edit["new"] = json!("a=2");
    invalid.push(vec![edit]);
    let mut edit = base.clone();
    edit["replacements"] = json!([]);
    invalid.push(vec![edit]);
    let mut edit = base.clone();
    edit["replacements"] = json!(vec![json!({"old":"a=1","new":"a=2"}); 129]);
    invalid.push(vec![edit]);
    let mut edit = base.clone();
    edit["replacements"][0]["new"] = json!("\0");
    invalid.push(vec![edit]);
    let mut edit = base.clone();
    edit["replacements"][0]["new"] = json!("x".repeat(2 * 1024 * 1024));
    invalid.push(vec![edit]);
    let mut edit = base.clone();
    edit["replacements"][0]["expected_hash"] = json!("ignored?");
    invalid.push(vec![edit]);
    let mut edit = base.clone();
    edit["unexpected"] = json!(true);
    invalid.push(vec![edit]);
    let mut conflict = base.clone();
    conflict["expected_hash"] = json!("other");
    invalid.push(vec![base.clone(), conflict]);
    invalid.push(vec![
        base.clone(),
        json!({"path":"src/a.py","expected_hash":snapshot.files["src/a.py"].hash,"source":"whole"}),
    ]);
    for files in invalid {
        let batch = json!({"snapshot":snapshot.hash,"jobs":[{"id":"a","summary":"Scoped edits","files":files}]});
        assert!(f.report("preview", batch.clone()).is_err());
        assert!(f.report("apply", batch).is_err());
        assert_eq!(f.snapshot(), snapshot.hash);
    }
}

#[test]
fn invalid_grouped_proposals_never_persist_or_advance_running_attempts() {
    let f = Fixture::new();
    f.start("2");
    f.plan(json!([job("a", "src", &[])]), json!([])).unwrap();
    let attempt = f.reserve("a");
    let snapshot = f.project.snapshot().unwrap();
    for edit in [
        json!({"path":"src/new.py","expected_hash":null,"replacements":[{"old":"missing","new":"new"}]}),
        json!({"path":"src/a.py","expected_hash":"stale","replacements":[{"old":"a=1","new":"a=2"}]}),
        json!({"path":"request.txt","expected_hash":snapshot.files["request.txt"].hash,"replacements":[{"old":"feature","new":"change"}]}),
        json!({"path":"src/a.py","expected_hash":snapshot.files["src/a.py"].hash,"replacements":[{"old":"a=1","new":"a=2"},{"old":"a=1","new":"a=3"}]}),
    ] {
        assert!(f.report("attempt-propose", json!({"attempt":attempt["id"],"snapshot":attempt["snapshot"],"summary":"Scoped proposal","edits":[edit]})).is_err());
        let state = f.run(&["status", "demo"]).unwrap();
        assert_eq!(state["attempts"][0]["status"], "running");
        assert_eq!(state["task_states"]["a"]["status"], "running");
        assert_eq!(f.snapshot(), snapshot.hash);
        assert!(!f.root.join("src/new.py").exists());
        assert!(f
            .report("attempt-apply", json!({"attempts":[attempt["id"]]}))
            .is_err());
    }
}

#[test]
fn advisory_architecture_acceptance_persists_independent_review_without_repair() {
    let f = Fixture::new();
    f.start("3");
    let contract = architecture_contract();
    f.architecture_plan(
        json!([job("a", "src/a.py", &[]), job("b", "src/b.py", &[])]),
        contract.clone(),
    )
    .unwrap();
    f.apply(&[("a", "src/a.py"), ("b", "src/b.py")]).unwrap();
    let report = architecture_assessment(json!([architecture_issue("a", "src/a.py", "advisory")]));
    let reviewed = f.report("review", json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"tasks":[],"architecture":report})).unwrap();
    assert_eq!(reviewed["phase"], "qa");
    assert_eq!(reviewed["architecture"], contract);
    assert_eq!(reviewed["round"], 1);
    assert_eq!(reviewed["task_states"]["a"]["status"], "applied");
    assert_eq!(reviewed["task_states"]["b"]["status"], "applied");
    assert_eq!(reviewed["findings"], json!([]));
    assert_eq!(reviewed["architecture_report_fresh"], true);
    let done = f.report("qa", json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"tasks":[],"proofs":[],
        "architecture":{"assessment":["QA assessed executed checks, separately from independent review."],"issues":[]}})).unwrap();
    assert_eq!(done["phase"], "done");
    assert_eq!(done["architecture_report"], reviewed["architecture_report"]);
    let resumed = Project::open(&f.root).unwrap();
    let restored: Value =
        serde_json::from_str(&workflow::run(&resumed, &["status".into(), "demo".into()]).unwrap())
            .unwrap();
    assert_eq!(
        restored["architecture_report"],
        reviewed["architecture_report"]
    );
    assert_eq!(restored["architecture_report_fresh"], true);
    fs::write(f.root.join("src/a.py"), "a=9\n").unwrap();
    assert_eq!(
        f.run(&["status", "demo"]).unwrap()["architecture_report_fresh"],
        false
    );
}

#[test]
fn architecture_only_blocker_repairs_its_task_and_consumers_but_preserves_sibling() {
    let f = Fixture::new();
    f.start("3");
    f.architecture_plan(
        json!([
            job("a", "src/a.py", &[]),
            job("b", "src/b.py", &[]),
            job("consumer", "src/a.py", &["a"])
        ]),
        architecture_contract(),
    )
    .unwrap();
    f.apply(&[("a", "src/a.py"), ("b", "src/b.py")]).unwrap();
    f.apply(&[("consumer", "src/a.py")]).unwrap();
    assert!(f
        .report(
            "review",
            json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"tasks":[]})
        )
        .is_err());
    let report = architecture_assessment(json!([
        architecture_issue("a", "src/a.py", "blocking"),
        architecture_issue("b", "src/b.py", "advisory")
    ]));
    assert!(f.report("review", json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"tasks":[],"architecture":report})).is_err());
    let source = f.snapshot();
    let repaired = f.report("review", json!({"snapshot":source,"verdict":"changes","findings":[],"tasks":[],"architecture":report})).unwrap();
    assert_eq!(repaired["phase"], "developing");
    assert_eq!(repaired["round"], 2);
    assert_eq!(repaired["task_states"]["a"]["status"], "pending");
    assert_eq!(repaired["task_states"]["consumer"]["status"], "pending");
    assert_eq!(repaired["task_states"]["b"]["status"], "applied");
    assert_eq!(repaired["findings"].as_array().unwrap().len(), 1);
    let finding = repaired["findings"][0].as_str().unwrap();
    for part in ["src/a.py:1", "Consequence:", "Fix:", "Verification:"] {
        assert!(finding.contains(part));
    }
    assert!(!finding.contains("src/b.py"));
    assert_eq!(f.snapshot(), source);
    assert_eq!(repaired["architecture_report_fresh"], true);
}

#[test]
fn single_qa_requires_architecture_and_repairs_architecture_only_blockers() {
    let f = Fixture::new();
    f.run(&[
        "start",
        "demo",
        "--request",
        f.root.join("request.txt").to_str().unwrap(),
        "--mode",
        "single",
        "--rounds",
        "3",
    ])
    .unwrap();
    f.architecture_plan(json!([job("change", "src", &[])]), architecture_contract())
        .unwrap();
    f.apply(&[("change", "src/a.py")]).unwrap();
    let mut qa = json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"tasks":[],"proofs":[],"assessment":["Changed source matches the requested value; automated checks are not configured."]});
    assert!(f.report("qa", qa.clone()).is_err());
    qa["architecture"] = architecture_assessment(json!([architecture_issue(
        "change", "src/a.py", "blocking"
    )]));
    assert!(f.report("qa", qa.clone()).is_err());
    qa["verdict"] = json!("changes");
    let repair = f.report("qa", qa.clone()).unwrap();
    assert_eq!(repair["phase"], "developing");
    assert_eq!(repair["task_states"]["change"]["status"], "pending");
    assert_eq!(
        repair["architecture_report"]["issues"][0]["severity"],
        "blocking"
    );
    f.apply(&[("change", "src/a.py")]).unwrap();
    qa["snapshot"] = json!(f.snapshot());
    qa["verdict"] = json!("accept");
    qa["architecture"] = architecture_assessment(json!([]));
    let done = f.report("qa", qa).unwrap();
    assert_eq!(done["phase"], "done");
    assert_eq!(done["architecture_report"]["issues"], json!([]));
    assert_eq!(done["architecture_report_fresh"], true);
}

#[test]
fn architecture_issues_require_complete_source_grounding_and_affected_write_scopes() {
    let f = Fixture::new();
    f.start("3");
    f.architecture_plan(json!([job("a", "src/a.py", &[])]), architecture_contract())
        .unwrap();
    f.apply(&[("a", "src/a.py")]).unwrap();
    let source = f.snapshot();
    let base = architecture_issue("a", "src/a.py", "blocking");
    let mut invalid = Vec::new();
    for (field, value) in [
        ("severity", json!("important")),
        ("tasks", json!([])),
        ("tasks", json!(["missing"])),
        ("tasks", json!(["a", "a"])),
        ("path", json!("src/b.py")),
        ("path", json!("src/missing.py")),
        ("path", json!("../outside.py")),
        ("path", json!("src")),
        ("path", json!("x".repeat(513))),
        ("line", json!(0)),
        ("line", json!(999)),
        ("line", json!(-1)),
        ("problem", json!("   ")),
        ("verification", json!("")),
        ("fix", json!("x".repeat(769))),
        ("consequence", json!("\0")),
    ] {
        let mut issue = base.clone();
        issue[field] = value;
        invalid.push(architecture_assessment(json!([issue])));
    }
    let mut issue = base.clone();
    issue.as_object_mut().unwrap().remove("consequence");
    invalid.push(architecture_assessment(json!([issue])));
    let mut issue = base.clone();
    issue["extra"] = json!("not allowed");
    invalid.push(architecture_assessment(json!([issue])));
    invalid.push(json!({"assessment":[],"issues":[]}));
    invalid.push(json!({"assessment":vec!["Evidence";9],"issues":[]}));
    invalid.push(json!({"assessment":["я".repeat(769)],"issues":[]}));
    invalid.push(architecture_assessment(json!(vec![base; 9])));
    for report in invalid {
        assert!(f.report("review", json!({"snapshot":source,"verdict":"changes","findings":[],"tasks":[],"architecture":report})).is_err());
        let state = f.run(&["status", "demo"]).unwrap();
        assert_eq!(state["phase"], "reviewing");
        assert!(state["architecture_report"].is_null());
        assert_eq!(f.snapshot(), source);
    }
}

#[test]
fn unicode_architecture_evidence_uses_character_limits_without_truncating_canonical_findings() {
    let f = Fixture::new();
    f.start("3");
    f.architecture_plan(json!([job("a", "src/a.py", &[])]), architecture_contract())
        .unwrap();
    f.apply(&[("a", "src/a.py")]).unwrap();
    let source = f.snapshot();
    let mut issue = architecture_issue("a", "src/a.py", "blocking");
    for (field, letter) in [
        ("problem", "П"),
        ("consequence", "С"),
        ("fix", "И"),
        ("verification", "Т"),
    ] {
        issue[field] = json!(letter.repeat(768));
    }
    let report = json!({"assessment":["Я".repeat(768)],"issues":[issue]});
    for field in [
        "problem",
        "consequence",
        "fix",
        "verification",
        "assessment",
    ] {
        let mut invalid = report.clone();
        if field == "assessment" {
            invalid[field] = json!(["Я".repeat(769)]);
        } else {
            invalid["issues"][0][field] = json!("Я".repeat(769));
        }
        assert!(f.report("review", json!({"snapshot":source,"verdict":"changes","findings":[],"tasks":[],"architecture":invalid})).is_err());
    }
    // The new structured form must not relax limits for legacy free-form text.
    for findings in [
        json!(["Я".repeat(2049)]),
        json!(["x".repeat(4097)]),
        json!(vec!["Legacy finding"; 64]),
    ] {
        assert!(f.report("review", json!({"snapshot":source,"verdict":"changes","findings":findings,"tasks":["a"],"architecture":report})).is_err());
        assert_eq!(f.run(&["status", "demo"]).unwrap()["phase"], "reviewing");
    }
    let repaired = f.report("review", json!({"snapshot":source,"verdict":"changes","findings":[],"tasks":[],"architecture":report})).unwrap();
    assert_eq!(repaired["phase"], "developing");
    assert_eq!(
        repaired["architecture_report"]["assessment"],
        report["assessment"]
    );
    assert_eq!(repaired["architecture_report"]["issues"], report["issues"]);
    let finding = repaired["findings"][0].as_str().unwrap();
    assert!(finding.len() > 4096);
    for field in ["problem", "consequence", "fix", "verification"] {
        assert!(finding.contains(report["issues"][0][field].as_str().unwrap()));
    }
}

#[test]
fn extension_contract_bounds_are_validated_without_turning_future_paths_into_write_rules() {
    let f = Fixture::new();
    f.start("3");
    let base = architecture_contract();
    let mut invalid = Vec::new();
    for (field, value) in [
        ("boundaries", json!([])),
        ("boundaries", json!(vec!["Boundary"; 9])),
        ("boundaries", json!(["я".repeat(1025)])),
        ("dependencies", json!(vec!["Dependency"; 9])),
        ("dependencies", json!(["\0"])),
        (
            "extension_scenarios",
            json!(vec![base["extension_scenarios"][0].clone(); 4]),
        ),
    ] {
        let mut contract = base.clone();
        contract[field] = value;
        invalid.push(contract);
    }
    for (field, value) in [
        ("change", json!("")),
        ("change", json!("x".repeat(1025))),
        ("expected_paths", json!([])),
        ("expected_paths", json!(vec!["src/a.py"; 17])),
        ("preserved_paths", json!(vec!["src/b.py"; 17])),
        ("expected_paths", json!(["src/a.py", "src/a.py"])),
        ("expected_paths", json!(["src/*.py"])),
        ("preserved_paths", json!(["src/"])),
        ("expected_paths", json!(["../outside"])),
        ("expected_paths", json!(["x".repeat(513)])),
    ] {
        let mut contract = base.clone();
        contract["extension_scenarios"][0][field] = value;
        invalid.push(contract);
    }
    for contract in invalid {
        assert!(f
            .architecture_plan(json!([job("a", "src/a.py", &[])]), contract)
            .is_err());
        assert_eq!(f.run(&["status", "demo"]).unwrap()["phase"], "planning");
    }
    let mut valid = base;
    valid["boundaries"] = json!(["я".repeat(1024)]);
    valid["extension_scenarios"][0]["expected_paths"] = json!(["future/plugin"]);
    f.architecture_plan(json!([job("a", "src/a.py", &[])]), valid)
        .unwrap();
    // preserved_paths describes a hypothetical future change, not this task's permissions.
    f.apply(&[("a", "src/a.py")]).unwrap();
    assert!(fs::read_to_string(f.root.join("src/a.py"))
        .unwrap()
        .contains("# edited"));
    let small = Fixture::new();
    small.start("2");
    small.architecture_plan(json!([job("a", "src/a.py", &[])]), json!({"boundaries":["Existing boundary is retained."],"dependencies":[],"extension_scenarios":[]})).unwrap();
}

#[test]
fn architecture_contract_changes_invalidate_applied_progress_even_without_source_changes() {
    let f = Fixture::new();
    f.start("5");
    let tasks = json!([job("a", "src/a.py", &[]), job("b", "src/b.py", &[])]);
    let contract = architecture_contract();
    f.architecture_plan(tasks.clone(), contract.clone())
        .unwrap();
    f.apply(&[("a", "src/a.py"), ("b", "src/b.py")]).unwrap();
    f.report("review", json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"tasks":[],"architecture":architecture_assessment(json!([]))})).unwrap();
    let source = f.snapshot();
    f.run(&["replan", "demo"]).unwrap();
    let unchanged = f
        .architecture_plan(tasks.clone(), contract.clone())
        .unwrap();
    assert_eq!(unchanged["phase"], "reviewing");
    assert_eq!(unchanged["task_states"]["a"]["status"], "applied");
    assert_eq!(unchanged["architecture_report_fresh"], true);
    f.run(&["replan", "demo"]).unwrap();
    let mut changed = contract;
    changed["boundaries"] = json!(["State ownership moves behind a new service interface."]);
    let revised = f.architecture_plan(tasks, changed).unwrap();
    assert_eq!(revised["phase"], "developing");
    assert_eq!(revised["task_states"]["a"]["status"], "pending");
    assert_eq!(revised["task_states"]["b"]["status"], "pending");
    assert_eq!(revised["architecture_report_fresh"], false);
    assert!(!revised["architecture_report"].is_null());
    assert_eq!(f.snapshot(), source);
}

#[test]
fn team_qa_architecture_blockers_repair_but_do_not_replace_independent_review_evidence() {
    let f = Fixture::new();
    f.start("3");
    f.architecture_plan(
        json!([job("a", "src/a.py", &[]), job("b", "src/b.py", &[])]),
        architecture_contract(),
    )
    .unwrap();
    f.apply(&[("a", "src/a.py"), ("b", "src/b.py")]).unwrap();
    let reviewed = f.report("review", json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"tasks":[],"architecture":architecture_assessment(json!([]))})).unwrap();
    let repaired = f.report("qa", json!({"snapshot":f.snapshot(),"verdict":"changes","findings":[],"tasks":[],"proofs":[],"architecture":architecture_assessment(json!([architecture_issue("a", "src/a.py", "blocking")]))})).unwrap();
    assert_eq!(repaired["phase"], "developing");
    assert_eq!(repaired["task_states"]["a"]["status"], "pending");
    assert_eq!(repaired["task_states"]["b"]["status"], "applied");
    assert_eq!(
        repaired["architecture_report"],
        reviewed["architecture_report"]
    );
}

#[test]
fn legacy_durable_plan_without_architecture_fields_still_resumes_and_completes() {
    let f = Fixture::new();
    f.start("2");
    f.plan(json!([job("a", "src/a.py", &[])]), json!([]))
        .unwrap();
    let path = f.project.state.join("workflows/demo.json");
    let mut state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    state.as_object_mut().unwrap().remove("last_architecture");
    state.as_object_mut().unwrap().remove("last_verification");
    state["plan"]
        .as_object_mut()
        .unwrap()
        .remove("architecture");
    state["plan"]
        .as_object_mut()
        .unwrap()
        .remove("verification");
    fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
    assert!(f.run(&["status", "demo"]).unwrap()["architecture"].is_null());
    let restored = f.run(&["status", "demo"]).unwrap();
    assert_eq!(restored["verification"], json!([]));
    assert!(restored["verification_report"].is_null());
    f.apply(&[("a", "src/a.py")]).unwrap();
    f.report(
        "review",
        json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[]}),
    )
    .unwrap();
    assert_eq!(
        f.report(
            "qa",
            json!({"snapshot":f.snapshot(),"verdict":"accept","findings":[],"proofs":[]})
        )
        .unwrap()["phase"],
        "done"
    );
}
