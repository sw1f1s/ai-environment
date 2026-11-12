//! A real compiler graph must expose contract consumers without duplicating source.
use aienv::{brief, graph, project::Project};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ae-impact-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        for (path, text) in [
            ("pyproject.toml", "[project]\nname='impact_fixture'\nversion='0.1.0'\n"),
            ("src/__init__.py", ""),
            ("src/model.py", "class Money:\n    cents: int\n"),
            ("src/policy.py", "from src.model import Money\ndef quote(amount: Money) -> Money:\n    return amount # REUSE_BODY_SENTINEL\n"),
            ("src/consumer.py", "from src.model import Money\nfrom src.policy import quote as priced\ndef run(amount: Money) -> Money:\n    return priced(amount) # CONSUMER_BODY_SENTINEL\n"),
            ("tests/test_policy.py", "from src.policy import quote\nfrom src.model import Money\ndef test_price():\n    assert quote(Money()) is not None\n"),
        ] { fs::write(root.join(path), text).unwrap(); }
        fs::write(root.join("src/padding.py"), "# padding\n".repeat(2000)).unwrap();
        Project::init(&root).unwrap();
        Self(root)
    }
    fn project(&self) -> Project {
        Project::open(&self.0).unwrap()
    }
    fn build(&self) {
        graph::run(
            &self.project(),
            &strings(&[
                "build",
                "--language",
                "python",
                "--project",
                "pyproject.toml",
            ]),
        )
        .unwrap();
    }
    fn packet(&self, args: &[&str]) -> String {
        brief::run(&self.project(), &strings(args)).unwrap()
    }
    fn cache_bytes(&self) -> Vec<u8> {
        fs::read(self.0.join(".ai-env/semantic/python.json")).unwrap()
    }
    fn alter_cache(&self, change: impl FnOnce(&mut serde_json::Value)) {
        let mut value = serde_json::from_slice(&self.cache_bytes()).unwrap();
        change(&mut value);
        fs::write(
            self.0.join(".ai-env/semantic/python.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

#[test]
fn impact_shows_real_alias_callers_types_tests_and_missing_scope_without_source_duplication() {
    let f = Fixture::new();
    f.build();
    let packet = f.packet(&["--impact", "quote", "--path", "src", "--budget", "9000"]);
    assert!(packet.len() <= 9000, "{packet}");
    assert_eq!(packet.matches("REUSE_BODY_SENTINEL").count(), 1, "{packet}");
    assert_eq!(
        packet.matches("CONSUMER_BODY_SENTINEL").count(),
        1,
        "{packet}"
    );
    assert!(
        packet.contains("reuse-candidate function src.policy.quote @src/policy.py:2"),
        "{packet}"
    );
    assert!(
        packet.contains("calls src.consumer.run -> src.policy.quote @src/consumer.py:4"),
        "{packet}"
    );
    assert!(
        packet.contains("uses_type") && packet.contains("Money"),
        "{packet}"
    );
    assert!(
        packet.contains("calls(test-path)")
            && packet.contains("@tests/test_policy.py:4 [outside-scope"),
        "{packet}"
    );
    assert!(
        !packet.contains("assert quote(Money()) is not None"),
        "{packet}"
    );
    assert!(
        packet.contains("test_paths_linked=1/1") && packet.contains("not test results"),
        "{packet}"
    );
}

#[test]
fn impact_is_source_bound_and_bounded_when_relevant_evidence_does_not_fit() {
    let f = Fixture::new();
    f.build();
    let packet = f.packet(&["--impact", "quote", "--path", "src", "--budget", "2048"]);
    assert!(
        packet.len() <= 2048 && packet.contains("impact coverage"),
        "{packet}"
    );
    assert!(!packet.contains("rows_omitted=0"), "{packet}");
    fs::write(f.0.join("src/policy.py"), "def new_policy(): return 42\n").unwrap();
    // An ordinary brief never starts an analyzer or consumes stale facts.
    let before = f.cache_bytes();
    let current = f.packet(&["quote", "--path", "src", "--budget", "4096"]);
    assert!(current.contains("lexical selection only"), "{current}");
    assert!(
        !current.contains("src.policy.quote @") && !current.contains("calls(test-path)"),
        "{current}"
    );
    assert!(current.contains("new_policy"), "{current}");
    assert_eq!(before, f.cache_bytes());
}

#[test]
fn large_impact_refreshes_only_source_changes_with_recorded_options_and_current_facts() {
    let f = Fixture::new();
    f.build();
    let before: graph::Envelope = serde_json::from_slice(&f.cache_bytes()).unwrap();
    // Source additions/deletions and shrinking a consumer invalidate old source
    // extents. Old coordinates must not be treated as describing current code.
    fs::remove_file(f.0.join("src/policy.py")).unwrap();
    fs::write(
        f.0.join("src/release_policy.py"),
        "def repriced(): return 42\n",
    )
    .unwrap();
    fs::write(
        f.0.join("src/consumer.py"),
        "from src.release_policy import repriced\ndef run(): return repriced()\n",
    )
    .unwrap();
    let packet = f.packet(&[
        "--impact",
        "repriced quote",
        "--path",
        "src",
        "--budget",
        "9000",
    ]);
    assert!(
        packet.len() <= 9000 && packet.contains("refreshed source changes"),
        "{packet}"
    );
    assert!(
        packet.contains("calls src.consumer.run -> src.release_policy.repriced"),
        "{packet}"
    );
    assert!(
        !packet.contains("src.policy.quote @") && !packet.contains("relationships unknown"),
        "{packet}"
    );
    let after = graph::load_current(&f.project(), "python").unwrap();
    assert_eq!(before.spec.options, after.spec.options);
    assert_ne!(before.source_key, after.source_key);
    assert!(after
        .source_hashes
        .as_ref()
        .unwrap()
        .contains_key("src/release_policy.py"));
    assert!(!after
        .source_hashes
        .as_ref()
        .unwrap()
        .contains_key("src/policy.py"));
    let published = f.cache_bytes();
    assert!(!f
        .packet(&["--impact", "repriced", "--path", "src", "--budget", "9000"])
        .contains("refreshed source changes"));
    assert_eq!(
        published,
        f.cache_bytes(),
        "A fresh graph must not be rebuilt"
    );
}

#[test]
fn small_or_missing_graph_packets_never_refresh_or_discover_a_graph() {
    let f = Fixture::new();
    let missing = f.packet(&["--impact", "quote", "--budget", "4096"]);
    assert!(missing.contains("relationships unknown"));
    assert!(!f.0.join(".ai-env/semantic/python.json").exists());
    f.build();
    let before = f.cache_bytes();
    fs::write(f.0.join("src/policy.py"), "def quote(): return 7\n").unwrap();
    let args = ["quote", "--path", "src/policy.py", "--budget", "4096"];
    let small = f.packet(&args);
    assert!(small.contains("brief sources") && !small.contains("graph python"));
    let enriched = f.packet(&[
        "--impact",
        "quote",
        "--path",
        "src/policy.py",
        "--budget",
        "4096",
    ]);
    assert!(enriched.contains("brief sources") && !enriched.contains("graph python"));
    assert_eq!(enriched.matches("def quote(): return 7").count(), 1);
    assert!(enriched.contains("not_full=0"));
    assert!(enriched.contains("implementation-candidate @src/policy.py:1 (in-packet)"));
    assert_eq!(before, f.cache_bytes());
}

#[test]
fn source_drift_cannot_hide_corrupt_manifest_root_backend_or_configuration_provenance() {
    for cause in [
        "manifest",
        "graph",
        "root",
        "backend",
        "configuration",
        "external",
        "legacy",
    ] {
        let f = Fixture::new();
        f.build();
        match cause {
            "manifest" => f.alter_cache(|cache| {
                cache["source_hashes"]["src/policy.py"] = "0".repeat(64).into()
            }),
            "graph" => {
                f.alter_cache(|cache| cache["graph"]["nodes"][0]["name"] = "invented".into())
            }
            "root" => f.alter_cache(|cache| cache["project_root"] = "/different/project".into()),
            "backend" => f.alter_cache(|cache| cache["backend_hash"] = "0".repeat(64).into()),
            "configuration" => {
                fs::write(f.0.join("pyproject.toml"), "[project]\nname='changed'\n").unwrap()
            }
            "external" => {
                let path = f.0.join(".ai-env/external-metadata");
                fs::write(&path, "before").unwrap();
                f.alter_cache(|cache| {
                    cache["graph"]["inputs"]
                        .as_array_mut()
                        .unwrap()
                        .push(serde_json::json!({
                            "path":path, "hash":aienv::project::sha("before")
                        }));
                    let graph: graph::Graph =
                        serde_json::from_value(cache["graph"].clone()).unwrap();
                    cache["index_hash"] = graph::model::graph_hash(&graph).unwrap().into();
                });
                fs::write(path, "after").unwrap();
            }
            "legacy" => f.alter_cache(|cache| {
                cache.as_object_mut().unwrap().remove("source_hashes");
            }),
            _ => unreachable!(),
        }
        let before = f.cache_bytes();
        if cause == "legacy" {
            assert!(
                graph::load_current(&f.project(), "python").is_ok(),
                "A fresh old cache remains readable"
            );
        }
        fs::write(f.0.join("src/policy.py"), "def quote(): return 8\n").unwrap();
        let packet = f.packet(&["--impact", "quote", "--budget", "4096"]);
        assert!(
            packet.contains("lexical selection only") && packet.contains("relationships unknown"),
            "{cause}: {packet}"
        );
        assert!(
            !packet.contains("refreshed source changes") && !packet.contains("reuse-candidate"),
            "{cause}: {packet}"
        );
        assert_eq!(
            before,
            f.cache_bytes(),
            "Unsafe cache must remain untouched: {cause}"
        );
    }
}

#[test]
fn failed_refreshes_preserve_old_caches_and_share_two_attempt_limit() {
    let f = Fixture::new();
    // A trusted test adapter exercises controller failure/attempt limits without
    // waiting 20s or altering the real bundled compiler. Semantic fact tests
    // above use the actual Python analyzer.
    let scripts = f.0.join(".trusted-adapter/analyzers");
    fs::create_dir_all(&scripts).unwrap();
    fs::create_dir_all(f.0.join(".trusted-adapter/bin/roslyn")).unwrap();
    fs::write(
        f.0.join(".trusted-adapter/bin/roslyn/GraphAnalyzer.dll"),
        "fixture",
    )
    .unwrap();
    let helper = "import json,sys\nfrom pathlib import Path\nr=json.load(sys.stdin)\nroot=Path(r['root'])\nmode=root/'.ai-env/adapter-mode'\nif mode.exists():\n    with (root/'.ai-env/attempts').open('a') as out: out.write(r['language']+'\\n')\n    if mode.read_text() == 'fail':\n        print('controlled adapter failure',file=sys.stderr)\n        sys.exit(1)\n    (root/'src/app.c').write_text('int quote(void) { return 9; }\\n')\nprint(json.dumps(dict(schema_version=1,language=r['language'],analyzer=dict(name='controller test fixture',version='1',capabilities=[]),nodes=[],edges=[],diagnostics=[],unresolved=[],inputs=[],analysis_complete=False,limitations=[])))\n";
    fs::write(scripts.join("clang.py"), helper).unwrap();
    fs::write(scripts.join("csharp.py"), helper).unwrap();
    fs::write(
        f.0.join("src/app.c"),
        format!(
            "int quote(void) {{ return 1; }}\n{}",
            "// padding\n".repeat(2000)
        ),
    )
    .unwrap();
    fs::write(
        f.0.join("src/App.cs"),
        "class App { static int Quote() => 1; }\n",
    )
    .unwrap();
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_aienv"))
            .args(["--root", f.0.to_str().unwrap()])
            .args(args)
            .env("AI_ENV_ANALYZERS", &scripts)
            .output()
            .unwrap()
    };
    let mut old = Vec::new();
    for language in ["c", "cpp", "csharp"] {
        let built = cli(&["graph", "build", "--language", language]);
        assert!(
            built.status.success(),
            "{}",
            String::from_utf8_lossy(&built.stderr)
        );
        let path = f.0.join(format!(".ai-env/semantic/{language}.json"));
        old.push((path.clone(), fs::read(path).unwrap()));
    }
    fs::write(
        f.0.join("src/app.c"),
        format!(
            "int quote(void) {{ return 2; }}\n{}",
            "// padding\n".repeat(2000)
        ),
    )
    .unwrap();
    fs::write(
        f.0.join("src/App.cs"),
        "class App { static int Quote() => 2; }\n",
    )
    .unwrap();
    fs::write(f.0.join(".ai-env/adapter-mode"), "fail").unwrap();
    let failed = cli(&["brief", "--impact", "quote", "--budget", "9000"]);
    assert!(
        failed.status.success(),
        "{}",
        String::from_utf8_lossy(&failed.stderr)
    );
    let packet = String::from_utf8(failed.stdout).unwrap();
    assert!(
        packet.contains("controlled adapter failure")
            && packet.contains("attempt limit (2) reached"),
        "{packet}"
    );
    assert!(
        packet.contains("relationships unknown") && !packet.contains("refreshed source changes"),
        "{packet}"
    );
    assert_eq!(
        fs::read_to_string(f.0.join(".ai-env/attempts")).unwrap(),
        "c\ncpp\n"
    );
    for (path, bytes) in &old {
        assert_eq!(fs::read(path).unwrap(), *bytes);
    }

    // The same trusted adapter now mutates the source during analysis. No new
    // graph or source-bound packet may be published for the old snapshot.
    fs::write(f.0.join(".ai-env/adapter-mode"), "race").unwrap();
    let raced = cli(&["brief", "--impact", "quote", "--budget", "9000"]);
    assert!(!raced.status.success());
    assert!(raced.stdout.is_empty());
    assert!(String::from_utf8_lossy(&raced.stderr).contains("changed while preparing brief"));
    for (path, bytes) in &old {
        assert_eq!(fs::read(path).unwrap(), *bytes);
    }
}

#[test]
fn impact_names_unresolved_call_sites_without_inventing_a_target() {
    let f = Fixture::new();
    fs::write(
        f.0.join("src/policy.py"),
        "def quote(amount):\n    return unknown_runtime_hook(amount)\n",
    )
    .unwrap();
    f.build();
    let packet = f.packet(&["--impact", "quote", "--path", "src", "--budget", "12000"]);
    assert!(packet.contains("graph python: PARTIAL"), "{packet}");
    assert!(
        packet.contains("unresolved calls @src/policy.py:2:"),
        "{packet}"
    );
    assert!(
        packet.contains("dynamic dispatch is not guessed"),
        "{packet}"
    );
    assert!(!packet.contains("-> unknown_runtime_hook"), "{packet}");
    assert!(packet.len() <= 12000);
}

#[test]
fn broad_query_preserves_caller_type_reuse_and_unresolved_evidence_under_one_budget() {
    let f = Fixture::new();
    let mut many = String::from("from src.model import Money\n");
    for i in 0..96 {
        let body = if i == 0 {
            "unknown_runtime_hook(amount)"
        } else {
            "amount"
        };
        many.push_str(&format!("def quote_{i:03}(amount: Money) -> Money:\n    return {body} # extra definition padding to require source omission\n"));
    }
    fs::write(f.0.join("src/a_many.py"), many).unwrap();
    f.build();
    let packet = f.packet(&["--impact", "quote", "--path", "src", "--budget", "9000"]);
    assert!(packet.len() <= 9000, "{packet}");
    let impact = packet.split("\nimpact:").nth(1).unwrap();
    let seeds: usize = impact
        .split("impact coverage seeds=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(seeds > 64, "{impact}");
    assert!(
        impact.contains("calls src.consumer.run -> src.policy.quote @src/consumer.py:4 [in-packet"),
        "{impact}"
    );
    assert_eq!(
        packet.matches("CONSUMER_BODY_SENTINEL").count(),
        1,
        "{packet}"
    );
    assert!(
        impact.contains("uses_type") && impact.contains("src.model.Money"),
        "{impact}"
    );
    assert!(impact.contains("reuse-candidate"), "{impact}");
    assert!(
        impact.contains("unresolved calls @src/a_many.py:3:"),
        "{impact}"
    );
    assert!(!impact.contains("rows_omitted=0"), "{impact}");
}

#[test]
fn impact_retains_a_late_caller_before_early_generic_matches() {
    let f = Fixture::new();
    let consumer = fs::read_to_string(f.0.join("src/consumer.py")).unwrap();
    fs::write(
        f.0.join("src/consumer.py"),
        format!("{}{consumer}", "# quote context\n".repeat(2000)),
    )
    .unwrap();
    f.build();
    let packet = f.packet(&["--impact", "quote", "--path", "src", "--budget", "9000"]);
    assert!(
        packet.len() <= 9000 && packet.contains("CONSUMER_BODY_SENTINEL"),
        "{packet}"
    );
    assert!(
        packet.contains("@src/consumer.py:2004 [line-in-packet"),
        "{packet}"
    );
}

#[test]
fn impact_discloses_when_a_real_caller_line_is_missing_from_a_partial_file() {
    let f = Fixture::new();
    let consumer = fs::read_to_string(f.0.join("src/consumer.py")).unwrap();
    // The call site itself cannot fit, even when graph hints choose it first.
    // Keep the read-gap test meaningful after late-line prioritization.
    let consumer = consumer.replace(
        "CONSUMER_BODY_SENTINEL",
        &format!("CONSUMER_BODY_SENTINEL {}", "large ".repeat(3000)),
    );
    fs::write(
        f.0.join("src/consumer.py"),
        format!("{}{consumer}", "# quote context\n".repeat(2000)),
    )
    .unwrap();
    f.build();
    let packet = f.packet(&["--impact", "quote", "--path", "src", "--budget", "9000"]);
    assert!(packet.len() <= 9000, "{packet}");
    assert!(packet.contains("[src/consumer.py excerpt"), "{packet}");
    assert!(!packet.contains("CONSUMER_BODY_SENTINEL"), "{packet}");
    let impact = packet.split("\nimpact:").nth(1).unwrap();
    assert!(
        impact.contains(
            "calls src.consumer.run -> src.policy.quote @src/consumer.py:2004 [line-missing"
        ),
        "{impact}"
    );
}
