use aienv::{
    model_plans,
    project::{sha, FileEntry, Project, Snapshot},
    workflow,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

fn make_snapshot(paths: &[&str]) -> Snapshot {
    let files = paths
        .iter()
        .map(|path| {
            let text = format!("// {path}: Привет 😀\n");
            (
                (*path).to_owned(),
                FileEntry {
                    path: (*path).to_owned(),
                    hash: sha(text.as_bytes()),
                    text,
                },
            )
        })
        .collect();
    Snapshot {
        hash: sha(serde_json::to_vec(paths).unwrap()),
        files,
        omitted: 0,
    }
}
fn capture_base(snapshot: &Snapshot) -> Value {
    model_plans::capture(snapshot, "").unwrap()
}
fn reference(base: &Value, path: &str) -> String {
    model_plans::public(base).unwrap()["refs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["path"] == path)
        .unwrap_or_else(|| panic!("missing reference for {path}"))["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn task(id: &str, paths: &[&str], read_refs: &[&str], depends: &[&str]) -> Value {
    json!({"id":id,"title":"Reuse existing behavior","instructions":"Change only the requested behavior and preserve caller invariants",
        "paths":paths,"read_refs":read_refs,"depends":depends})
}
fn answer(snapshot: &Snapshot, tasks: Vec<Value>, owners: &[&str]) -> Value {
    json!({"snapshot":snapshot.hash,"mode":"single","summary":"Implementation proposal",
        "tasks":tasks,"checks":[],"acceptance":["Requested behavior and preservation boundaries work"],
        "verification":[{"id":"boundaries","acceptance":[0],"tasks":owners,"risk":"Rejection must preserve previous state and use the shared policy","checks":[]}],
        "architecture":{"boundaries":[],"dependencies":[],"extension_scenarios":[]},
        "edits":[{"path":"src/consumer.rs","source":"updated();\n"}]})
}
fn bind(snapshot: &Snapshot, base: &Value, answer: &Value) -> Value {
    model_plans::bind(answer, base, &snapshot.hash).unwrap()
}
fn resign(base: &mut Value) {
    let fields = json!([
        base["schema"],
        base["snapshot"],
        base["files"],
        base["refs"],
        base["omitted_scopes"],
        base["indexed_omitted"],
        base["source_digest"]
    ]);
    let mut bytes = b"ai-env-model-plan-base-1\0".to_vec();
    bytes.extend(serde_json::to_vec(&fields).unwrap());
    base["digest"] = json!(sha(bytes));
}

#[test]
fn visible_brief_files_and_ancestors_precede_other_scopes_in_a_bounded_catalog() {
    let names = (0..350)
        .map(|n| format!("aaa/file_{n:03}.rs"))
        .chain(["zzz/深い/visible.rs".into()])
        .collect::<Vec<_>>();
    let refs = names.iter().map(String::as_str).collect::<Vec<_>>();
    let mut snapshot = make_snapshot(&refs);
    snapshot.omitted = 7;
    let source = format!("snapshot {}\nbrief selected sources\n[zzz/深い/visible.rs excerpt; omitted lines require source inspection]\n12:visible();\n",snapshot.hash);
    let captured = model_plans::capture(&snapshot, &source).unwrap();
    assert_eq!(captured, model_plans::capture(&snapshot, &source).unwrap());
    let public = model_plans::public(&captured).unwrap();
    let rows = public["refs"].as_array().unwrap();
    assert_eq!(rows[0]["path"], "zzz/深い/visible.rs");
    assert_eq!(rows[1]["path"], "zzz/深い");
    assert_eq!(rows[2]["path"], "zzz");
    assert_eq!(rows.len(), 128);
    assert_eq!(public["omitted_scopes"], 226);
    assert_eq!(public["indexed_omitted"], 7);
    assert!(serde_json::to_vec(&public).unwrap().len() <= 16 * 1024);
    assert!(rows.iter().all(|r| r["path"] != "."));
    let ids = rows
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), rows.len());
    assert!(ids
        .iter()
        .all(|id| id.len() <= 12 && id.bytes().all(|b| b.is_ascii_alphanumeric())));
    for key in ["files", "source_digest", "snapshot", "digest", "schema"] {
        assert!(
            public.get(key).is_none(),
            "private field in public catalog: {key}"
        );
    }
    assert!(!public.to_string().contains(&snapshot.hash));
}

#[test]
fn full_header_priority_and_byte_cap_do_not_claim_omitted_scopes_are_visible() {
    let names = (0..150)
        .map(|n| {
            format!(
                "{}/{}/{}/file_{n:03}.rs",
                "long".repeat(30),
                "nested".repeat(20),
                "path".repeat(25)
            )
        })
        .collect::<Vec<_>>();
    let refs = names.iter().map(String::as_str).collect::<Vec<_>>();
    let snapshot = make_snapshot(&refs);
    let visible = names.last().unwrap();
    let source = format!(
        "snapshot {}\n[{}:1..2 hash={}]\nsource data\n",
        snapshot.hash, visible, snapshot.files[visible].hash
    );
    let public = model_plans::public(&model_plans::capture(&snapshot, &source).unwrap()).unwrap();
    assert_eq!(public["refs"][0]["path"], *visible);
    assert!(
        public["refs"].as_array().unwrap().len() < 128,
        "long paths should hit the byte cap"
    );
    assert!(serde_json::to_vec(&public).unwrap().len() <= 16 * 1024);
    assert_eq!(
        public["refs"].as_array().unwrap().len()
            + public["omitted_scopes"].as_u64().unwrap() as usize,
        153
    );
    let non_header = format!(
        "snapshot {}\nmentioned {} but not shown\n[unindexed.py:1..9]\n",
        snapshot.hash, visible
    );
    let other =
        model_plans::public(&model_plans::capture(&snapshot, &non_header).unwrap()).unwrap();
    assert_ne!(other["refs"][0]["path"], *visible);
}

#[test]
fn read_reference_binding_derives_exact_scope_unions_without_touching_writes_or_edits() {
    let snapshot = make_snapshot(&[
        "src/consumer.rs",
        "src/policy.rs",
        "docs/invariants.md",
        "other.rs",
    ]);
    let base = capture_base(&snapshot);
    let policy = reference(&base, "src/policy.rs");
    let docs = reference(&base, "docs");
    let model = answer(
        &snapshot,
        vec![task(
            "change",
            &[
                "src/consumer.rs",
                "src/generated",
                "src/generated/result.rs",
            ],
            &[&policy, &docs],
            &[],
        )],
        &["change"],
    );
    let bound = bind(&snapshot, &base, &model);
    assert_eq!(
        bound["tasks"][0]["context"],
        json!(["docs", "src/policy.rs"])
    );
    assert_eq!(bound["tasks"][0]["paths"], model["tasks"][0]["paths"]);
    assert_eq!(
        bound["verification"][0]["paths"],
        json!(["docs", "src/consumer.rs", "src/generated", "src/policy.rs"])
    );
    assert_eq!(bound["edits"], model["edits"]);
    assert_eq!(bound["architecture"], model["architecture"]);
    assert!(bound["tasks"][0].get("read_refs").is_none());
    assert!(
        model["tasks"][0].get("context").is_none(),
        "saved answer must stay unmodified"
    );
    assert!(model["verification"][0].get("paths").is_none());
    assert!(
        !bound["verification"][0]["paths"]
            .as_array()
            .unwrap()
            .contains(&json!("src")),
        "never invent a wider parent"
    );
}

#[test]
fn provider_dependencies_add_future_write_scopes_only_to_direct_consumers() {
    let snapshot = make_snapshot(&["src/consumer.rs", "src/policy.rs"]);
    let base = capture_base(&snapshot);
    let model = answer(
        &snapshot,
        vec![
            task("provider", &["src/new shared/интерфейс.rs"], &[], &[]),
            task("adapter", &["src/adapter.rs"], &[], &["provider"]),
            task("consumer", &["src/consumer.rs"], &[], &["adapter"]),
        ],
        &["consumer"],
    );
    let bound = bind(&snapshot, &base, &model);
    assert_eq!(bound["tasks"][0]["context"], json!([]));
    assert_eq!(
        bound["tasks"][1]["context"],
        json!(["src/new shared/интерфейс.rs"])
    );
    assert_eq!(bound["tasks"][2]["context"], json!(["src/adapter.rs"]));
    assert_eq!(
        bound["verification"][0]["paths"],
        json!(["src/adapter.rs", "src/consumer.rs"])
    );
    assert!(model_plans::public(&base).unwrap()["refs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["path"] != "src/new shared/интерфейс.rs"));
    let mut missing_dependency = model.clone();
    missing_dependency["tasks"][1]["depends"] = json!(["future-unknown"]);
    assert!(model_plans::bind(&missing_dependency, &base, &snapshot.hash).is_err());
}

#[test]
fn prose_context_and_model_verification_paths_are_structurally_rejected() {
    let snapshot = make_snapshot(&["src/consumer.rs", "check.py"]);
    let base = capture_base(&snapshot);
    let model = answer(
        &snapshot,
        vec![task("change", &["src/consumer.rs"], &[], &[])],
        &["change"],
    );
    for prose in [
        "Approved check test runs /private/tmp/ai-env-maintenance-upstream/venv/bin/python check.py.",
        "Approved check test maps exactly to /private/tmp/ai-env-maintenance-upstream/venv/bin/python check.py.",
    ] {
        let mut invalid=model.clone();
        invalid["tasks"][0]["context"]=json!([prose]);
        assert!(model_plans::bind(&invalid,&base,&snapshot.hash).unwrap_err().to_string().contains("never context"));
        invalid["tasks"][0].as_object_mut().unwrap().remove("context");
        invalid["tasks"][0]["read_refs"]=json!([prose]);
        assert!(model_plans::bind(&invalid,&base,&snapshot.hash).is_err());
    }
    for paths in [json!(["outside.rs"]), json!([]), Value::Null] {
        let mut invalid = model.clone();
        invalid["verification"][0]["paths"] = paths;
        assert!(model_plans::bind(&invalid, &base, &snapshot.hash)
            .unwrap_err()
            .to_string()
            .contains("omit paths"));
    }
    assert_eq!(
        bind(&snapshot, &base, &model)["verification"][0]["paths"],
        json!(["src/consumer.rs"])
    );
}

#[test]
fn unknown_duplicate_cyclic_or_unordered_ownership_never_becomes_a_plan() {
    let snapshot = make_snapshot(&["src/consumer.rs", "src/policy.rs"]);
    let base = capture_base(&snapshot);
    let read = reference(&base, "src/policy.rs");
    let good = answer(
        &snapshot,
        vec![task("change", &["src/consumer.rs"], &[&read], &[])],
        &["change"],
    );
    let mut cases = Vec::new();
    for (key, value) in [
        ("id", json!("bad task id")),
        ("read_refs", json!([read, read])),
        ("read_refs", json!(["unknown"])),
        ("paths", json!(["src/consumer.rs", "src/consumer.rs"])),
        ("depends", json!(["unknown"])),
        ("depends", json!(["change"])),
        ("depends", json!(["unknown", "unknown"])),
    ] {
        let mut invalid = good.clone();
        invalid["tasks"][0][key] = value;
        cases.push(invalid);
    }
    let mut duplicate = good.clone();
    duplicate["tasks"]
        .as_array_mut()
        .unwrap()
        .push(good["tasks"][0].clone());
    cases.push(duplicate);
    cases.push(answer(
        &snapshot,
        vec![
            task("a", &["a.rs"], &[], &["b"]),
            task("b", &["b.rs"], &[], &["a"]),
        ],
        &["a"],
    ));
    cases.push(answer(
        &snapshot,
        vec![
            task("a", &["src"], &[], &[]),
            task("b", &["src/policy.rs"], &[], &[]),
        ],
        &["a"],
    ));
    cases.push(answer(
        &snapshot,
        vec![
            task(
                "a",
                &["src/consumer.rs"],
                &[&reference(&base, "src/policy.rs")],
                &[],
            ),
            task("b", &["src/policy.rs"], &[], &[]),
        ],
        &["a"],
    ));
    for (index, invalid) in cases.into_iter().enumerate() {
        assert!(
            model_plans::bind(&invalid, &base, &snapshot.hash).is_err(),
            "case {index}"
        );
    }
    let ordered = answer(
        &snapshot,
        vec![
            task("a", &["src/policy.rs"], &[], &[]),
            task(
                "b",
                &["src/consumer.rs"],
                &[&reference(&base, "src/policy.rs")],
                &["a"],
            ),
        ],
        &["b"],
    );
    assert!(model_plans::bind(&ordered, &base, &snapshot.hash).is_ok());
    let ordered_writes = answer(
        &snapshot,
        vec![
            task("provider", &["src/new"], &[], &[]),
            task("consumer", &["src/new/interface.rs"], &[], &["provider"]),
        ],
        &["consumer"],
    );
    assert!(model_plans::bind(&ordered_writes, &base, &snapshot.hash).is_ok());
}

#[test]
fn obligation_owners_checks_and_acceptance_coverage_are_validated_before_deriving_paths() {
    let snapshot = make_snapshot(&["src/consumer.rs"]);
    let base = capture_base(&snapshot);
    let good = answer(
        &snapshot,
        vec![task("change", &["src/consumer.rs"], &[], &[])],
        &["change"],
    );
    for (key, value) in [
        ("id", json!("bad id")),
        ("tasks", json!(["unknown"])),
        ("tasks", json!(["change", "change"])),
        ("tasks", json!([])),
        ("acceptance", json!([])),
        ("acceptance", json!([1])),
        ("acceptance", json!([0, 0])),
        ("risk", json!(" ")),
        ("risk", json!("я".repeat(769))),
        ("checks", json!(["unapproved"])),
    ] {
        let mut invalid = good.clone();
        invalid["verification"][0][key] = value;
        assert!(
            model_plans::bind(&invalid, &base, &snapshot.hash).is_err(),
            "accepted {key}"
        );
    }
    let mut uncovered = good.clone();
    uncovered["acceptance"] = json!(["one", "two"]);
    assert!(model_plans::bind(&uncovered, &base, &snapshot.hash).is_err());
    let mut duplicate = good.clone();
    duplicate["verification"]
        .as_array_mut()
        .unwrap()
        .push(good["verification"][0].clone());
    assert!(model_plans::bind(&duplicate, &base, &snapshot.hash).is_err());
    let mut checked = good.clone();
    checked["checks"] = json!(["test"]);
    assert!(model_plans::bind(&checked, &base, &snapshot.hash).is_err());
    checked["verification"][0]["checks"] = json!(["test"]);
    checked["verification"][0]["risk"] = json!("я".repeat(768));
    assert!(model_plans::bind(&checked, &base, &snapshot.hash).is_ok());
}

#[test]
fn derived_scope_limit_is_explicit_and_never_silently_discards_read_evidence() {
    let names = (0..18)
        .map(|n| format!("file_{n:02}.rs"))
        .collect::<Vec<_>>();
    let refs = names.iter().map(String::as_str).collect::<Vec<_>>();
    let snapshot = make_snapshot(&refs);
    let base = capture_base(&snapshot);
    let selected = names[1..]
        .iter()
        .map(|p| reference(&base, p))
        .collect::<Vec<_>>();
    let selected_refs = selected.iter().map(String::as_str).collect::<Vec<_>>();
    let mut model = answer(
        &snapshot,
        vec![task("change", &[&names[0]], &selected_refs, &[])],
        &["change"],
    );
    assert!(model_plans::bind(&model, &base, &snapshot.hash)
        .unwrap_err()
        .to_string()
        .contains("exceed 16"));
    model["tasks"][0]["read_refs"] = json!(selected[..15]);
    assert_eq!(
        bind(&snapshot, &base, &model)["verification"][0]["paths"]
            .as_array()
            .unwrap()
            .len(),
        16
    );
    assert_eq!(model["tasks"][0]["read_refs"].as_array().unwrap().len(), 15);
}

#[test]
fn canonical_future_paths_allow_spaces_and_unicode_but_reject_globs_traversal_and_file_ancestors() {
    let snapshot = make_snapshot(&["src/consumer.rs"]);
    let base = capture_base(&snapshot);
    for path in ["HANDOFF.md", "src/new folder/новый.rs", "новый проект.rs"] {
        let model = answer(
            &snapshot,
            vec![task("change", &[path], &[], &[])],
            &["change"],
        );
        assert_eq!(
            bind(&snapshot, &base, &model)["tasks"][0]["paths"],
            json!([path])
        );
    }
    for path in [
        "./src/x.rs",
        "src//x.rs",
        "src/../x.rs",
        "/x.rs",
        "src/",
        "src/*.rs",
        "src/x?.rs",
        "src/[x].rs",
        "src\\x.rs",
        ".ai-env/state.json",
        ".git/config",
        ".env",
        "src/consumer.rs/child.rs",
    ] {
        let model = answer(
            &snapshot,
            vec![task("change", &[path], &[], &[])],
            &["change"],
        );
        assert!(
            model_plans::bind(&model, &base, &snapshot.hash).is_err(),
            "accepted {path}"
        );
    }
    let empty = make_snapshot(&[]);
    let empty_base = capture_base(&empty);
    assert_eq!(model_plans::public(&empty_base).unwrap()["refs"], json!([]));
    assert!(model_plans::bind(
        &answer(
            &empty,
            vec![task("init", &["src/new.rs"], &[], &[])],
            &["init"]
        ),
        &empty_base,
        &empty.hash
    )
    .is_ok());
}

#[test]
fn private_catalog_rejects_mixups_corruption_and_even_rechecksummed_invalid_references() {
    let snapshot = make_snapshot(&["src/consumer.rs"]);
    let base = capture_base(&snapshot);
    let good = answer(
        &snapshot,
        vec![task("change", &["src/consumer.rs"], &[], &[])],
        &["change"],
    );
    for invalid in [
        Value::Null,
        json!({}),
        json!({"snapshot":snapshot.hash,"refs":[]}),
    ] {
        assert!(model_plans::public(&invalid).is_err());
        assert!(model_plans::bind(&good, &invalid, &snapshot.hash).is_err());
    }
    for (key, value) in [
        ("schema", json!(2)),
        ("snapshot", json!("a".repeat(64))),
        ("source_digest", json!("b".repeat(64))),
        ("digest", json!("0".repeat(64))),
        ("omitted_scopes", json!(100)),
        ("extra", json!(true)),
    ] {
        let mut invalid = base.clone();
        invalid[key] = value;
        assert!(model_plans::public(&invalid).is_err(), "accepted {key}");
    }
    for (key, value) in [
        ("path", json!("unindexed.rs")),
        ("path", json!(".")),
        ("id", json!("invented")),
    ] {
        let mut invalid = base.clone();
        invalid["refs"][0][key] = value;
        resign(&mut invalid);
        assert!(
            model_plans::public(&invalid).is_err(),
            "accepted corrupt ref {key}"
        );
    }
    let mut duplicate = base.clone();
    duplicate["refs"][1] = duplicate["refs"][0].clone();
    resign(&mut duplicate);
    assert!(model_plans::public(&duplicate).is_err());
    assert!(model_plans::bind(&good, &base, &"c".repeat(64)).is_err());
    let mut stale = good.clone();
    stale["snapshot"] = json!("c".repeat(64));
    assert!(model_plans::bind(&stale, &base, &snapshot.hash).is_err());
    assert!(model_plans::capture(&snapshot, &format!("snapshot {}\n", "c".repeat(64))).is_err());
    let mut inconsistent = snapshot.clone();
    inconsistent.files.get_mut("src/consumer.rs").unwrap().text = "changed".into();
    assert!(model_plans::capture(&inconsistent, "").is_err());
    let mut inconsistent = snapshot.clone();
    inconsistent.files.get_mut("src/consumer.rs").unwrap().path = "other.rs".into();
    assert!(model_plans::capture(&inconsistent, "").is_err());
}

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
    project: Project,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ae-model-plans-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        let project = Project::init(&root).unwrap();
        fs::write(root.join("src/consumer.rs"), "consumer();\n").unwrap();
        fs::write(root.join("src/policy.rs"), "policy();\n").unwrap();
        fs::write(
            root.join("request.txt"),
            "Update consumer and preserve policy",
        )
        .unwrap();
        Self { root, project }
    }
    fn report(&self, action: &str, value: Value) -> aienv::Result<Value> {
        let path = self.root.with_extension("report.json");
        fs::write(&path, serde_json::to_vec(&value)?)?;
        let result = workflow::run(
            &self.project,
            &[
                action.into(),
                "demo".into(),
                "--file".into(),
                path.to_str().unwrap().into(),
            ],
        );
        fs::remove_file(path)?;
        Ok(serde_json::from_str(&result?)?)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn durable_replay_keeps_dispatch_refs_and_read_scopes_never_grant_write_permission() {
    let f = Fixture::new();
    let snapshot = f.project.snapshot().unwrap();
    let base = capture_base(&snapshot);
    let read = reference(&base, "src/policy.rs");
    let model = answer(
        &snapshot,
        vec![task("change", &["src/consumer.rs"], &[&read], &[])],
        &["change"],
    );
    f.project
        .write_state("dispatch-plan.json", &json!({"plan_base":base}))
        .unwrap();
    let resumed = Project::open(&f.root).unwrap();
    let persisted: Value = resumed.read_state("dispatch-plan.json").unwrap();
    let bound = bind(&snapshot, &persisted["plan_base"], &model);
    workflow::run(
        &f.project,
        &[
            "start".into(),
            "demo".into(),
            "--request".into(),
            f.root.join("request.txt").to_str().unwrap().into(),
        ],
    )
    .unwrap();
    f.report("plan",json!({"snapshot":bound["snapshot"],"tasks":bound["tasks"],"acceptance":bound["acceptance"],"checks":bound["checks"],"verification":bound["verification"]})).unwrap();
    let patch = |path: &str| json!({"snapshot":snapshot.hash,"jobs":[{"id":"change","summary":"Update requested behavior","files":[{"path":path,"expected_hash":snapshot.files[path].hash,"source":"updated();\n"}]}]});
    f.report("preview", patch("src/consumer.rs")).unwrap();
    assert!(
        f.report("preview", patch("src/policy.rs")).is_err(),
        "read references must not become write permission"
    );
    fs::write(f.root.join("src/policy.rs"), "external();\n").unwrap();
    assert_eq!(
        model_plans::bind(&model, &persisted["plan_base"], &snapshot.hash).unwrap(),
        bound,
        "binding must not read current source"
    );
    let latest = f.project.snapshot().unwrap();
    assert_ne!(snapshot.hash, latest.hash);
    assert!(
        model_plans::bind(
            &model,
            &model_plans::capture(&latest, "").unwrap(),
            &snapshot.hash
        )
        .is_err(),
        "never recapture a base on replay"
    );
    assert!(
        f.report("preview", patch("src/consumer.rs")).is_err(),
        "controller retains live snapshot authority"
    );
}
