use aienv::{
    edit, model_edits,
    project::{sha, Project, Snapshot, MAX_FILE},
    workflow,
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
            "ae-model-edits-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        let project = Project::init(&root).unwrap();
        fs::write(root.join("src/a.py"), "value = 1\r\nkeep = '漢😀'\r\n").unwrap();
        fs::write(root.join("outside.py"), "protected = 1\n").unwrap();
        fs::write(
            root.join("request.txt"),
            "Update the existing scoped implementation",
        )
        .unwrap();
        Self { root, project }
    }
    fn snapshot(&self) -> Snapshot {
        self.project.snapshot().unwrap()
    }
    fn apply(&self, snapshot: &str, edits: Value) -> aienv::Result<Value> {
        let path = self.root.with_extension("patch.json");
        fs::write(
            &path,
            serde_json::to_vec(&json!({"snapshot":snapshot,"edits":edits}))?,
        )?;
        let result = edit::run(
            &self.project,
            &["--patch".into(), path.to_str().unwrap().into()],
        );
        fs::remove_file(path)?;
        Ok(serde_json::from_str(&result?)?)
    }
    fn report(&self, action: &str, report: Value) -> aienv::Result<Value> {
        let path = self.root.with_extension("report.json");
        fs::write(&path, serde_json::to_vec(&report)?)?;
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
    fn plan(&self) {
        workflow::run(
            &self.project,
            &[
                "start".into(),
                "demo".into(),
                "--request".into(),
                self.root.join("request.txt").to_str().unwrap().into(),
            ],
        )
        .unwrap();
        self.report("plan", json!({"snapshot":self.snapshot().hash,"checks":[],"acceptance":["Scoped change works"],
            "tasks":[{"id":"change","title":"Update the existing source", "instructions":"Reuse the existing implementation", "paths":["src"],"context":[],"depends":[]}]})).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn unicode_updates_and_new_files_bind_original_hashes_without_model_hashes() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let base = model_edits::capture(&snapshot).unwrap();
    let edits = json!([
        {"path":"src/a.py","source":"value = 2\r\nkeep = '漢😀'\r\n"},
        {"path":"src/новый.py","source":"name = 'Привет 😀'\n"}
    ]);
    assert!(!edits.to_string().contains("expected_hash"));
    let bound = model_edits::bind(&edits, &base, &snapshot.hash).unwrap();
    assert_eq!(bound[0]["expected_hash"], snapshot.files["src/a.py"].hash);
    assert!(bound[1]["expected_hash"].is_null());
    f.apply(&snapshot.hash, bound).unwrap();
    assert_eq!(
        fs::read(f.root.join("src/a.py")).unwrap(),
        "value = 2\r\nkeep = '漢😀'\r\n".as_bytes()
    );
    assert_eq!(
        fs::read_to_string(f.root.join("src/новый.py")).unwrap(),
        "name = 'Привет 😀'\n"
    );
    assert!(
        !edits.to_string().contains("expected_hash"),
        "Binding must not mutate the saved model answer"
    );
}

#[test]
fn external_writer_cannot_be_rebased_into_an_existing_file_answer() {
    let f = Fixture::new();
    let dispatch = f.snapshot();
    let base = model_edits::capture(&dispatch).unwrap();
    fs::write(f.root.join("src/a.py"), "external = 3\n").unwrap();
    let current = f.snapshot();
    let bound = model_edits::bind(
        &json!([{"path":"src/a.py","source":"model = 2\n"}]),
        &base,
        &dispatch.hash,
    )
    .unwrap();
    assert_eq!(bound[0]["expected_hash"], dispatch.files["src/a.py"].hash);
    assert_ne!(bound[0]["expected_hash"], current.files["src/a.py"].hash);
    assert!(f
        .apply(&dispatch.hash, bound.clone())
        .unwrap_err()
        .to_string()
        .contains("Stale snapshot"));
    // Even an incorrectly rebased outer envelope cannot bless an old answer:
    // the per-file CAS still carries the dispatch hash, not the latest hash.
    assert!(f
        .apply(&current.hash, bound)
        .unwrap_err()
        .to_string()
        .contains("expected_hash"));
    assert_eq!(
        fs::read_to_string(f.root.join("src/a.py")).unwrap(),
        "external = 3\n"
    );
}

#[test]
fn newly_appearing_destination_and_deleted_original_are_never_treated_as_fresh() {
    for originally_present in [false, true] {
        let f = Fixture::new();
        let path = "src/raced.py";
        if originally_present {
            fs::write(f.root.join(path), "original = 1\n").unwrap();
        }
        let dispatch = f.snapshot();
        let base = model_edits::capture(&dispatch).unwrap();
        if originally_present {
            fs::remove_file(f.root.join(path)).unwrap();
        } else {
            fs::write(f.root.join(path), "external = 1\n").unwrap();
        }
        let current = f.snapshot();
        let bound = model_edits::bind(
            &json!([{"path":path,"source":"model = 2\n"}]),
            &base,
            &dispatch.hash,
        )
        .unwrap();
        if originally_present {
            assert_eq!(bound[0]["expected_hash"], sha("original = 1\n"));
        } else {
            assert!(bound[0]["expected_hash"].is_null());
        }
        assert!(f.apply(&current.hash, bound).is_err());
        assert_eq!(f.root.join(path).exists(), !originally_present);
        if !originally_present {
            assert_eq!(
                fs::read_to_string(f.root.join(path)).unwrap(),
                "external = 1\n"
            );
        }
    }
}

#[test]
fn replay_uses_the_durable_dispatch_base_and_rejects_a_recaptured_base() {
    let f = Fixture::new();
    let dispatch = f.snapshot();
    let base = model_edits::capture(&dispatch).unwrap();
    f.project
        .write_state("dispatch-inputs.json", &json!({"edit_base":base}))
        .unwrap();
    fs::write(f.root.join("src/a.py"), "external = 'later'\n").unwrap();
    let resumed = Project::open(&f.root).unwrap();
    let private: Value = resumed.read_state("dispatch-inputs.json").unwrap();
    let answer = json!([{"path":"src/a.py","source":"model = 2\n"}]);
    let rebound = model_edits::bind(&answer, &private["edit_base"], &dispatch.hash).unwrap();
    assert_eq!(rebound[0]["expected_hash"], dispatch.files["src/a.py"].hash);
    let latest = resumed.snapshot().unwrap();
    assert!(model_edits::bind(
        &answer,
        &model_edits::capture(&latest).unwrap(),
        &dispatch.hash
    )
    .is_err());
    assert!(f.apply(&latest.hash, rebound).is_err());
}

#[test]
fn strict_model_shapes_reject_hashes_legacy_edits_deletions_and_invalid_limits() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let base = model_edits::capture(&snapshot).unwrap();
    let oversized = "x".repeat(MAX_FILE as usize + 1);
    let bad = [
        Value::Null,
        json!({}),
        json!([null]),
        json!([{"path":"src/a.py","expected_hash":null,"source":"x"}]),
        json!([{"path":"src/a.py","expected_hash":snapshot.files["src/a.py"].hash,"source":"x"}]),
        json!([{"path":"src/a.py","source":"x","extra":true}]),
        json!([{"path":"src/a.py","source":"x","replacements":null}]),
        json!([{"path":"src/a.py","source":null,"replacements":[{"old":"value","new":"name"}]}]),
        json!([{"path":"src/a.py","old":"value","new":"name"}]),
        json!([{"path":"src/a.py","delete":true}]),
        json!([{"path":"src/a.py","source":"x"},{"path":"src/a.py","source":"y"}]),
        json!([{"path":"src/new.py","replacements":[{"old":"x","new":"y"}]}]),
        json!([{"path":"src/a.py","replacements":[]}]),
        json!([{"path":"src/a.py","replacements":vec![json!({"old":"x","new":"y"});129]}]),
        json!([{"path":"src/a.py","replacements":[{"old":"","new":"x"}]}]),
        json!([{"path":"src/a.py","replacements":[{"old":"x","new":"y","extra":true}]}]),
        json!([{"path":"src/a.py","replacements":[{"old":"x","new":"\0"}]}]),
        json!([{"path":"src/a.py","source":"\0"}]),
        json!([{"path":"src/a.py","source":oversized}]),
        json!(vec![json!({"path":"src/a.py","source":"x"}); 129]),
    ];
    for (index, edit) in bad.iter().enumerate() {
        assert!(
            model_edits::bind(edit, &base, &snapshot.hash).is_err(),
            "accepted invalid shape {index}"
        );
    }
    for path in [
        "./src/a.py",
        "src//a.py",
        "src/a.py/",
        "../a.py",
        "/a.py",
        "src/../a.py",
        "src\\a.py",
        ".ai-env/state.json",
        ".git/config",
        ".env",
        "src/a\0.py",
    ] {
        assert!(
            model_edits::bind(&json!([{"path":path,"source":"x"}]), &base, &snapshot.hash).is_err(),
            "accepted {path:?}"
        );
    }
    assert_eq!(
        model_edits::bind(&json!([]), &base, &snapshot.hash).unwrap(),
        json!([])
    );
    assert!(
        model_edits::bind(
            &json!([{"path":"src/a.py","source":""}]),
            &base,
            &snapshot.hash
        )
        .is_ok(),
        "Empty content means an empty file, never deletion"
    );
}

#[test]
fn missing_corrupt_and_mismatched_private_bases_fail_closed_even_for_noop_answers() {
    let f = Fixture::new();
    let snapshot = f.snapshot();
    let base = model_edits::capture(&snapshot).unwrap();
    for invalid in [
        Value::Null,
        json!({}),
        json!({"snapshot":snapshot.hash,"files":{}}),
    ] {
        assert!(model_edits::bind(&json!([]), &invalid, &snapshot.hash).is_err());
    }
    for (key, value) in [
        ("schema", json!(2)),
        ("snapshot", json!("a".repeat(64))),
        ("digest", json!("0".repeat(64))),
        ("digest", json!("invalid")),
        ("extra", json!(true)),
    ] {
        let mut invalid = base.clone();
        invalid[key] = value;
        assert!(
            model_edits::bind(&json!([]), &invalid, &snapshot.hash).is_err(),
            "accepted base mutation {key}"
        );
    }
    for hash in [
        "a".repeat(64),
        "F".repeat(64),
        "g".repeat(64),
        "f".repeat(63),
        String::new(),
    ] {
        let mut invalid = base.clone();
        invalid["files"]["src/a.py"] = json!(hash);
        assert!(model_edits::bind(&json!([]), &invalid, &snapshot.hash).is_err());
    }
    let mut invalid = base.clone();
    invalid["files"]["./src/a.py"] = json!(sha("x"));
    assert!(model_edits::bind(&json!([]), &invalid, &snapshot.hash).is_err());
    assert!(model_edits::bind(&json!([]), &base, &"a".repeat(64)).is_err());
    assert!(model_edits::bind(&json!([]), &base, "").is_err());
    let mut inconsistent = snapshot.clone();
    inconsistent.files.get_mut("src/a.py").unwrap().text = "tampered".into();
    assert!(model_edits::capture(&inconsistent).is_err());
    let mut inconsistent = snapshot.clone();
    inconsistent.files.get_mut("src/a.py").unwrap().path = "different.py".into();
    assert!(model_edits::capture(&inconsistent).is_err());
}

#[test]
fn workflow_retains_sequential_matching_scope_and_atomic_failure_authority() {
    let f = Fixture::new();
    f.plan();
    let snapshot = f.snapshot();
    let base = model_edits::capture(&snapshot).unwrap();
    let valid = json!([{"path":"src/a.py","replacements":[{"old":"value = 1","new":"value = 2"},{"old":"value = 2","new":"value = 3"}]}]);
    let bound = model_edits::bind(&valid, &base, &snapshot.hash).unwrap();
    assert_eq!(bound[0]["expected_hash"], snapshot.files["src/a.py"].hash);
    let batch = |files| json!({"snapshot":snapshot.hash,"jobs":[{"id":"change","summary":"Scoped edits", "files":files}]});
    f.report("preview", batch(bound.clone())).unwrap();
    let out_of_scope = model_edits::bind(
        &json!([{"path":"outside.py","source":"changed = 1\n"}]),
        &base,
        &snapshot.hash,
    )
    .unwrap();
    assert!(f.report("preview", batch(out_of_scope)).is_err());
    let ambiguous = model_edits::bind(
        &json!([{"path":"src/a.py","replacements":[{"old":" = ","new":" := "}]}]),
        &base,
        &snapshot.hash,
    )
    .unwrap();
    assert!(f.report("preview", batch(ambiguous)).is_err());
    let late_failure = model_edits::bind(&json!([{"path":"src/a.py","replacements":[{"old":"value = 1","new":"value = 2"},{"old":"missing","new":"no"}]}]), &base, &snapshot.hash).unwrap();
    assert!(f.report("apply", batch(late_failure)).is_err());
    assert_eq!(f.snapshot().hash, snapshot.hash);
    f.report("apply", batch(bound)).unwrap();
    assert_eq!(
        fs::read_to_string(f.root.join("src/a.py")).unwrap(),
        "value = 3\r\nkeep = '漢😀'\r\n"
    );
}
