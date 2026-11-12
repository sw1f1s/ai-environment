use aienv::{brief, project::Project};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "ae-brief-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(p.join("src")).unwrap();
        Project::init(&p).unwrap();
        Self(p)
    }
    fn write(&self, path: &str, text: &str) {
        fs::write(self.0.join(path), text).unwrap();
    }
    fn read(&self, args: &[&str]) -> String {
        brief::run(
            &Project::open(&self.0).unwrap(),
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn optional_hashes_support_exact_edits_without_changing_default_packet() {
    let f = Fixture::new();
    f.write("src/app.py", "print('雪')\n");
    let expected = aienv::project::sha("print('雪')\n");
    assert!(!f.read(&[]).contains(&format!("hash={expected}")));
    let packet = f.read(&["--hashes"]);
    assert!(packet.contains(&format!("hash={expected}")) && packet.contains("print('雪')"));
    f.write("src/app.py", &"# unrelated\n".repeat(2000));
    let packet = f.read(&["--hashes", "--budget", "2048"]);
    assert!(packet.len() <= 2048 && packet.contains("excerpt hash="));
}

#[test]
fn explicit_source_scope_preserves_handoff_and_exposes_omitted_inputs() {
    let f = Fixture::new();
    f.write("src/price.py", "def quote(): return 1\n");
    f.write("build.py", "BUILD_AUTOMATION_SENTINEL\n");
    f.write("HANDOFF.md", "Keep this decision.\n");
    let text = f.read(&["--path", "src"]);
    assert!(text.contains("return 1") && text.contains("Keep this decision."));
    assert!(text.contains("scope src") && text.contains("other_indexed=1"));
    assert!(!text.contains("BUILD_AUTOMATION_SENTINEL"));
    assert!(f.read(&[]).contains("BUILD_AUTOMATION_SENTINEL"));
    let p = Project::open(&f.0).unwrap();
    assert!(brief::run(&p, &["--path".into(), "../outside".into()]).is_err());
}

#[test]
fn scoped_packet_includes_small_local_check_driver_but_not_unrelated_automation() {
    let f = Fixture::new();
    f.write("src/app.py", "print(1)\n");
    f.write("check.py", "import build\nimport test_public\n");
    f.write("build.py", "UNREQUESTED_BUILD\n");
    for path in [
        "check.py".to_owned(),
        "./check.py".to_owned(),
        f.0.join("check.py").display().to_string(),
    ] {
        let config =
            serde_json::json!({"checks":{"test":{"argv":["python3",path],"timeout_seconds":10}}});
        f.write(".ai-env/config.json", &config.to_string());
        let text = f.read(&["--path", "src"]);
        assert!(text.contains("import test_public") && text.contains("print(1)"));
        assert!(!text.contains("UNREQUESTED_BUILD"));
    }
    f.write("check.py", &"# large check\n".repeat(1000));
    assert!(!f.read(&["--path", "src"]).contains("# large check"));
}

#[test]
fn small_packet_contains_real_sources_handoff_and_check_without_analysis() {
    let f = Fixture::new();
    f.write("src/price.py", "def quote(price):\n    return price\n");
    f.write("HANDOFF.md", "Use integer cents.\n");
    f.write(
        ".ai-env/config.json",
        r#"{"checks":{"test":{"argv":["python3","check.py"],"timeout_seconds":10}}}"#,
    );
    fs::create_dir_all(f.0.join(".ai-env/semantic")).unwrap();
    f.write(".ai-env/semantic/python.json", "not a graph");
    let text = f.read(&["quote"]);
    assert!(text.contains("return price") && text.contains("Use integer cents."));
    assert!(text.contains(r#"["python3","check.py"]"#));
    assert!(text.contains("not_full=0") && !text.contains("PARTIAL"));
    let impact = f.read(&["--impact", "quote"]);
    assert!(impact.contains("return price") && impact.contains("not_full=0"));
    assert!(impact.contains("implementation-candidate @src/price.py:1 (in-packet)"));
    assert!(
        !impact.contains("graph python:"),
        "Small source bypasses graph reads"
    );
    f.write("src/price.py", "def quote(price):\n    return price + 1\n");
    assert!(f.read(&["quote"]).contains("return price + 1"));
}
#[test]
fn bounded_selection_keeps_unicode_valid_and_discloses_incomplete_sources() {
    let f = Fixture::new();
    f.write(
        "src/large.py",
        &format!(
            "{}\ndef refund():\n    return '雪😀'\n{}",
            "# unrelated\n".repeat(600),
            "# more\n".repeat(600)
        ),
    );
    f.write("src/unused.py", &"# unrelated\n".repeat(700));
    let text = f.read(&["refund", "--budget", "2048"]);
    assert!(text.len() <= 2048);
    assert!(text.contains("refund") && text.contains("雪😀"));
    assert!(
        text.contains("excerpt") && text.contains("not_full:") && text.contains("src/large.py")
    );
    assert!(!text.contains("not_full=0"));
}
#[test]
fn stale_graph_is_not_used_as_a_source_of_relations() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join(".ai-env/semantic")).unwrap();
    f.write(".ai-env/semantic/python.json", "{\"wrong\":true}");
    f.write(
        "src/large.py",
        &format!("def refund(): return 9\n{}", "# unrelated\n".repeat(2000)),
    );
    let text = f.read(&["refund", "--budget", "2048"]);
    assert!(text.contains("lexical selection only") && text.contains("return 9"));
}
#[test]
fn invalid_arguments_and_hidden_inputs_are_not_silently_accepted() {
    let f = Fixture::new();
    f.write("src/.env", "DO_NOT_READ=secret");
    f.write("src/app.py", "print('ok')\n");
    f.write("asset.bin", "binary-like non-source");
    assert!(!f.read(&[]).contains("DO_NOT_READ"));
    let p = Project::open(&f.0).unwrap();
    for args in [
        vec!["--budget", "1"],
        vec!["--body"],
        vec!["hello\nworld"],
        vec!["--budget", "2048", "--budget", "4096"],
        vec!["--impact"],
        vec!["--impact", "quote", "--impact"],
    ] {
        assert!(brief::run(&p, &args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).is_err());
    }
}

#[test]
fn option_terminator_keeps_task_text_literal() {
    let f = Fixture::new();
    f.write("src/app.py", "# --root --hashes\nprint('ok')\n");
    let text = f.read(&["--impact", "--", "--root", "--hashes"]);
    assert!(
        text.contains("print('ok')") && !text.contains("hash="),
        "{text}"
    );
}

#[test]
fn impact_without_fresh_graph_discloses_unknown_relations_within_original_budget() {
    let f = Fixture::new();
    f.write(
        "src/large.py",
        &format!("def refund(): return 9\n{}", "# padding\n".repeat(2000)),
    );
    let text = f.read(&["--impact", "refund", "--budget", "2048"]);
    assert!(text.len() <= 2048 && text.contains("return 9"));
    assert!(text.contains("No fresh cached graph") && text.contains("relationships unknown"));
    assert!(!text.contains("reuse function"));
    fs::create_dir_all(f.0.join(".ai-env/semantic")).unwrap();
    f.write(".ai-env/semantic/python.json", "{\"wrong\":true}");
    let stale = f.read(&["--impact", "refund", "--budget", "2048"]);
    assert!(stale.len() <= 2048 && stale.contains("lexical selection only"));
    assert!(stale.contains("No fresh cached graph") && !stale.contains("reuse function"));
}

#[test]
fn many_unavailable_graphs_cannot_displace_all_source_from_a_tight_packet() {
    let f = Fixture::new();
    f.write(
        "src/large.py",
        &format!("def quote(): return 9\n{}", "# padding\n".repeat(2000)),
    );
    fs::create_dir_all(f.0.join(".ai-env/semantic")).unwrap();
    for language in aienv::graph::model::LANGUAGES {
        f.write(
            &format!(".ai-env/semantic/{language}.json"),
            "{\"wrong\":true}",
        );
    }
    let packet = f.read(&["--impact", "quote", "--budget", "2048"]);
    assert!(
        packet.len() <= 2048 && packet.contains("def quote(): return 9"),
        "{packet}"
    );
    assert!(
        packet.contains("relationships unknown") && packet.contains("graph metadata omitted="),
        "{packet}"
    );
    assert!(packet.contains("manual refresh/rebuild needed"), "{packet}");
    let graph_bytes: usize = packet
        .lines()
        .filter(|line| line.starts_with("graph "))
        .map(|line| line.len() + 1)
        .sum();
    assert!(graph_bytes <= 512, "{packet}");
}

#[test]
fn long_tests_and_handoff_cannot_displace_implementation_and_callers() {
    let f = Fixture::new();
    f.write(
        "src/pricing.py",
        &format!(
            "def quote(price):\n    return Quote(price)\n{}",
            "# quote implementation\n".repeat(1200)
        ),
    );
    f.write(
        "src/service.py",
        &format!(
            "def reserve(price):\n    return quote(price).total\n{}",
            "# quote caller\n".repeat(1200)
        ),
    );
    f.write(
        "src/test_regressions.py",
        &format!(
            "def test_quote():\n    assert quote(10).total == 10\n{}",
            "# quote release regression\n".repeat(2000)
        ),
    );
    f.write(
        "HANDOFF.md",
        &"quote release prior decisions\n".repeat(2000),
    );
    f.write(
        "check.py",
        "run_build()\nrun_regressions('src/test_regressions.py')\n",
    );
    f.write(
        ".ai-env/config.json",
        r#"{"checks":{"test":{"argv":["python3","check.py"],"timeout_seconds":10}}}"#,
    );
    let text = f.read(&[
        "--impact",
        "quote release",
        "--hashes",
        "--path",
        "src",
        "--budget",
        "6000",
    ]);
    assert!(text.len() <= 6000);
    for path in [
        "src/pricing.py",
        "src/service.py",
        "src/test_regressions.py",
        "check.py",
        "HANDOFF.md",
    ] {
        assert!(
            text.contains(&format!("[{path}")),
            "Missing block {path}: {text}"
        );
    }
    assert!(text.contains("return Quote(price)") && text.contains("return quote(price).total"));
    assert!(text.contains("assert quote(10).total") && text.contains("run_regressions"));
    assert!(
        text.contains("selection balances")
            && text.contains("omitted lines require source inspection")
    );
    assert!(text.contains("No fresh cached graph") && !text.contains("not_full=0"));
}

#[test]
fn test_only_scope_can_use_the_implementation_share_without_loading_other_source() {
    let f = Fixture::new();
    f.write("src/app.py", "DO_NOT_LOAD_OTHER_SOURCE\n");
    f.write(
        "src/test_regressions.py",
        &format!(
            "{}def test_target():\n    assert '雪😀'\n",
            "# padding\n".repeat(800)
        ),
    );
    let text = f.read(&[
        "test_target",
        "--path",
        "src/test_regressions.py",
        "--budget",
        "2048",
    ]);
    assert!(text.len() <= 2048 && text.contains("assert '雪😀'"));
    assert!(
        text.contains("[src/test_regressions.py excerpt")
            && !text.contains("DO_NOT_LOAD_OTHER_SOURCE")
    );
}

#[test]
fn a_medium_implementation_is_not_truncated_when_its_small_caller_also_fits() {
    let f = Fixture::new();
    let implementation = format!(
        "def target():\n    return 1\n{}",
        "# target note\n".repeat(190)
    );
    f.write("src/app.py", &implementation);
    f.write(
        "src/caller.py",
        "from app import target\nresult = target()\n",
    );
    let text = f.read(&["target", "--budget", "4096"]);
    assert!(text.len() <= 4096 && text.contains("brief selected sources"));
    assert!(
        text.contains(&implementation),
        "Unused space must preserve a full implementation: {text}"
    );
    assert!(text.contains("result = target()") && text.contains("not_full=0"));
}

#[test]
fn conventional_csharp_tests_do_not_crowd_out_application_files() {
    let f = Fixture::new();
    f.write(
        "src/Engine.cs",
        &format!(
            "class Engine {{ int Target() => 1; }}\n{}",
            "// Target logic\n".repeat(200)
        ),
    );
    f.write(
        "src/Latest.cs",
        "class Latest { int Target() => new Engine().Target(); }\n",
    );
    f.write(
        "src/EngineTests.cs",
        &format!("// Target assertions\n{}", "// Target test\n".repeat(2000)),
    );
    let text = f.read(&["Target", "--budget", "4096"]);
    assert!(text.len() <= 4096);
    for path in ["src/Engine.cs", "src/Latest.cs", "src/EngineTests.cs"] {
        assert!(text.contains(&format!("[{path}")), "Missing {path}: {text}");
    }
}

#[test]
fn consistency_packet_keeps_local_analogue_source_without_a_query_match() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("src/import")).unwrap();
    fs::create_dir_all(f.0.join("tests")).unwrap();
    f.write(
        "src/import/csv.py",
        &format!(
            "def import_vendor():\n    return pipeline.run()\n{}",
            "# padding\n".repeat(2000)
        ),
    );
    f.write(
        "src/import/json.py",
        "def convert_record():\n    return adapter.convert()  # LOCAL_ADAPTER_EXAMPLE\n",
    );
    f.write(
        "tests/test_csv.py",
        "def test_existing_adapter(fixture):\n    fixture.run()  # LOCAL_TEST_EXAMPLE\n",
    );
    for i in 0..25 {
        f.write(
            &format!("src/distractor_{i:02}.py"),
            &format!(
                "def import_vendor(): return 1\n{}",
                "# distractor\n".repeat(1000)
            ),
        );
    }
    let text = f.read(&["--impact", "import_vendor", "--budget", "8000"]);
    assert!(text.len() <= 8000);
    assert!(
        text.contains("LOCAL_ADAPTER_EXAMPLE") && text.contains("LOCAL_TEST_EXAMPLE"),
        "{text}"
    );
    assert!(
        text.contains("local-example-candidate @src/import/json.py:1 (in-packet)"),
        "{text}"
    );
    assert!(
        text.contains("test-candidate @tests/test_csv.py:1 (in-packet)"),
        "{text}"
    );
    assert!(!text.contains("hash="));
    let scoped = f.read(&[
        "--impact",
        "import_vendor",
        "--path",
        "src/import/csv.py",
        "--budget",
        "8000",
    ]);
    assert!(!scoped.contains("LOCAL_ADAPTER_EXAMPLE") && !scoped.contains("LOCAL_TEST_EXAMPLE"));
    assert!(
        !scoped.contains("@src/import/json.py"),
        "Never invent an uninspected example"
    );
}

#[test]
fn exact_late_contract_survives_generic_wrapper_words_and_tight_budgets() {
    let f = Fixture::new();
    let source=format!("// LateContract_extra run_operation_extra\n{}pub struct LateContract;\nimpl LateContract {{\n    pub fn run_operation() -> u32 {{ 42 }}\n}}\n{}", "// environment grammar parser checks\n".repeat(900), "// unrelated tail\n".repeat(500));
    f.write("src/owner.rs", &source);
    f.write(
        "src/environment_grammar_parser.rs",
        &"// environment grammar parser checks\n".repeat(1200),
    );
    for budget in ["2048", "4096", "16000"] {
        let text = f.read(&[
            "--impact",
            "environment grammar parser checks LateContract run_operation",
            "--budget",
            budget,
        ]);
        assert!(text.len() <= budget.parse::<usize>().unwrap());
        assert!(
            text.contains("pub struct LateContract;")
                && text.contains("pub fn run_operation() -> u32 { 42 }"),
            "{text}"
        );
        assert!(!text.contains("hash="));
        assert!(text.contains("[src/owner.rs excerpt"));
    }
    let scoped = f.read(&[
        "--impact",
        "LateContract run_operation",
        "--path",
        "src/environment_grammar_parser.rs",
        "--budget",
        "2048",
    ]);
    assert!(!scoped.contains("pub struct LateContract;") && !scoped.contains("@src/owner.rs"));
}
