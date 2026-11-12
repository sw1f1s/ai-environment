use aienv::{
    consistency,
    graph::{self, model},
    project::{sha, FileEntry},
    retrieval::NavigationQuery,
};
use std::collections::{BTreeMap, BTreeSet};

fn file(path: &str, text: &str) -> FileEntry {
    FileEntry {
        path: path.into(),
        hash: sha(text),
        text: text.into(),
    }
}
fn select(files: &[FileEntry], query: &[&str]) -> consistency::Selection {
    consistency::select(
        &files.iter().collect::<Vec<_>>(),
        &NavigationQuery::new(&query.join(" ")),
        &[],
        &BTreeSet::new(),
    )
}
#[test]
fn local_example_precedes_distant_same_name_without_claiming_equivalence() {
    let files = vec![
        file(
            "src/import/vendor_csv.py",
            "def vendor_csv_import():\n    return pipeline.run()\n",
        ),
        file(
            "src/import/vendor_json.py",
            "def convert_record():\n    return adapter.convert()\n",
        ),
        file("src/metrics/import.py", "def import_metrics(): return 1\n"),
    ];
    let selected = select(&files, &["vendor_csv", "import"]);
    let text = selected.render(&BTreeMap::new(), 3000, false);
    assert!(
        text.find("local-example-candidate @src/import/vendor_json.py")
            .unwrap()
            < text
                .find("implementation-candidate @src/metrics/import.py")
                .unwrap()
    );
    assert!(text.contains("names/locality are not ownership or equivalent contracts"));
    assert!(text.contains("line-missing"));
    assert!(selected.priority("src/import/vendor_json.py") >= 300);
    assert!(selected.hints.contains_key("src/import/vendor_json.py"));
}
#[test]
fn sibling_language_and_admitted_scope_are_respected() {
    let files = vec![
        file("src/import.py", "def import_vendor(): pass\n"),
        file("src/other.rs", "fn execute() {}\n"),
        file("src/neighbor.py", "def execute(): pass\n"),
    ];
    let selected = select(&files, &["import_vendor"]);
    let text = selected.render(&BTreeMap::new(), 3000, false);
    assert!(text.contains("local-example-candidate @src/neighbor.py"));
    assert!(!text.contains("@src/other.rs"));
    assert!(!text.contains("outside"));
}
#[test]
fn filename_tests_and_visibility_are_explicit_not_execution_evidence() {
    let files = vec![
        file("src/pricing.py", "def calculate(): return 2\n"),
        file(
            "tests/test_pricing.py",
            "def test_calculate(): assert True\n",
        ),
        file("tests/test_other.py", "def test_calculate(): pass\n"),
    ];
    let selected = select(&files, &["calculate"]);
    let mut covered = BTreeMap::new();
    covered.insert("src/pricing.py", None);
    covered.insert("tests/test_pricing.py", Some(BTreeSet::from([1])));
    let text = selected.render(&covered, 3000, false);
    assert!(text.contains("test-candidate @tests/test_pricing.py:1 (line-in-packet)"));
    assert!(text.contains("test discovery/execution unknown"));
    assert!(!text.contains("@tests/test_other.py"));
    assert!(text.contains("@src/pricing.py:1 (in-packet)"));
}
#[test]
fn no_match_does_not_assert_absence_and_drivers_are_not_owners() {
    let files = vec![
        file("src/data.py", "def process(): pass\n"),
        file("check.py", "task_keyword\n"),
        file("HANDOFF.md", "task_keyword\n"),
    ];
    let selected = consistency::select(
        &files.iter().collect::<Vec<_>>(),
        &NavigationQuery::new("task_keyword"),
        &[],
        &BTreeSet::from(["check.py".into()]),
    );
    let text = selected.render(&BTreeMap::new(), 2000, false);
    assert!(text.contains("existing capability remains unknown"));
    assert!(!text.contains("implementation-candidate"));
    assert_eq!(selected.priority("check.py"), 0);
    assert!(selected.hints.is_empty());
}
#[test]
fn utf8_budget_determinism_hash_opt_in_and_source_changes() {
    let files = vec![
        file("src/雪.py", "# 雪\ndef 雪(): pass\n"),
        file("src/例.py", "def run(): pass\n"),
    ];
    let selected = select(&files, &["雪"]);
    for budget in 0..1500 {
        let text = selected.render(&BTreeMap::new(), budget, true);
        assert!(
            text.len() <= budget,
            "budget={budget}, bytes={}",
            text.len()
        );
        assert_eq!(text, selected.render(&BTreeMap::new(), budget, true));
    }
    let before = selected.render(&BTreeMap::new(), 3000, true);
    assert!(before.contains(&files[0].hash));
    assert!(!selected
        .render(&BTreeMap::new(), 3000, false)
        .contains(&files[0].hash));
    let changed = vec![file("src/雪.py", "def 雪(): return 1\n")];
    let after = select(&changed, &["雪"]).render(&BTreeMap::new(), 3000, true);
    assert!(after.contains(&changed[0].hash));
    assert!(!after.contains(&files[0].hash));
}
fn node(id: &str, name: &str, path: &str) -> model::Node {
    model::Node {
        id: id.into(),
        name: name.into(),
        qualified_name: name.into(),
        kind: "function".into(),
        signature: String::new(),
        definition: true,
        external: false,
        implicit: false,
        path: Some(path.into()),
        line: 1,
        column: 1,
        end_line: 1,
        start_byte: 0,
        end_byte: 1,
    }
}
fn envelope(files: &[FileEntry]) -> graph::Envelope {
    graph::Envelope {
        schema_version: 1,
        project_root: String::new(),
        spec: model::Spec {
            language: "python".into(),
            options: model::Options::default(),
        },
        source_key: String::new(),
        source_hashes: None,
        backend_hash: String::new(),
        index_hash: String::new(),
        graph: graph::Graph {
            schema_version: 1,
            language: "python".into(),
            analyzer: model::Analyzer {
                name: "test".into(),
                version: "1".into(),
                capabilities: vec![],
            },
            nodes: vec![
                node("owner", "pricing", "src/pricing.py"),
                node("test", "exercise", "tests/test_special.py"),
            ],
            edges: vec![model::Edge {
                source: "test".into(),
                target: "owner".into(),
                kind: "calls".into(),
                path: "tests/test_special.py".into(),
                line: 1,
                column: 1,
                dispatch: "static".into(),
            }],
            diagnostics: vec![],
            unresolved: vec![],
            inputs: files
                .iter()
                .map(|f| model::Input {
                    path: f.path.clone(),
                    hash: f.hash.clone(),
                })
                .collect(),
            analysis_complete: false,
            limitations: vec![],
        },
    }
}
#[test]
fn static_test_relation_requires_current_input_hashes_and_stays_in_scope() {
    let files = vec![
        file("src/pricing.py", "def pricing(): return 2\n"),
        file("tests/test_special.py", "pricing()\n"),
    ];
    let graph = envelope(&files);
    let refs: Vec<_> = files.iter().collect();
    let selected = consistency::select(
        &refs,
        &NavigationQuery::new("pricing"),
        &[graph.clone()],
        &BTreeSet::new(),
    );
    let text = selected.render(&BTreeMap::new(), 3000, false);
    assert!(text.contains("graph-test-candidate @tests/test_special.py:1"));
    assert!(text.contains("partial static graph"));
    let mut missing_inputs = graph.clone();
    missing_inputs.graph.inputs.clear();
    let missing = consistency::select(
        &refs,
        &NavigationQuery::new("pricing"),
        &[missing_inputs],
        &BTreeSet::new(),
    );
    assert!(!missing
        .render(&BTreeMap::new(), 3000, false)
        .contains("graph-test-candidate"));
    let scope = consistency::select(
        &refs[..1],
        &NavigationQuery::new("pricing"),
        &[graph.clone()],
        &BTreeSet::new(),
    );
    assert!(!scope
        .render(&BTreeMap::new(), 3000, false)
        .contains("@tests/test_special.py"));
    let changed = vec![
        files[0].clone(),
        file("tests/test_special.py", "changed_pricing()\n"),
    ];
    let stale = consistency::select(
        &changed.iter().collect::<Vec<_>>(),
        &NavigationQuery::new("pricing"),
        &[graph],
        &BTreeSet::new(),
    );
    assert!(!stale
        .render(&BTreeMap::new(), 3000, false)
        .contains("graph-test-candidate"));
}

#[test]
fn exact_contract_candidate_outranks_wrapper_path_noise_and_focuses_late_source() {
    let files = vec![
        file(
            "environment/grammar/parser.rs",
            "// preserve tests\nfn parse() {}\n",
        ),
        file(
            "src/operations.rs",
            &format!(
                "{}pub fn run_operation() {{}}\n",
                "// environment run_operation_extra\n".repeat(70)
            ),
        ),
        file("src/neighbor.rs", "pub fn similar_operation() {}\n"),
    ];
    let selected = select(
        &files,
        &["environment", "grammar", "parser", "tests", "run_operation"],
    );
    let text = selected.render(&BTreeMap::new(), 3000, false);
    assert!(
        text.find("implementation-candidate @src/operations.rs:71")
            .unwrap()
            < text
                .find("implementation-candidate @environment/grammar/parser.rs")
                .unwrap()
    );
    assert!(text.contains("exact_ids=1"));
    assert!(selected.hints["src/operations.rs"].contains(&71));
}

#[test]
fn explicit_target_path_stays_inside_admitted_files() {
    let files = vec![
        file("src/target.rs", "fn existing() {}\n"),
        file("parser/grammar.rs", "fn parse() {}\n"),
    ];
    let query = NavigationQuery::new("Change src/target.rs parser grammar outside/owner.rs");
    let selected = consistency::select(
        &files.iter().collect::<Vec<_>>(),
        &query,
        &[],
        &BTreeSet::new(),
    );
    let text = selected.render(&BTreeMap::new(), 3000, false);
    assert!(
        text.find("implementation-candidate @src/target.rs")
            .unwrap()
            < text
                .find("implementation-candidate @parser/grammar.rs")
                .unwrap()
    );
    assert!(text.contains("explicit_path=true"));
    assert!(!selected.hints.contains_key("outside/owner.rs"));
}
