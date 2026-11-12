//! Independent public-CLI checks shared by the four new semantic adapters.
use serde_json::Value;
#[test]
fn brief_uses_fresh_compiler_callers_in_large_source_sets_and_rejects_stale_facts() {
    let f = Fixture::new("python");
    fs::write(f.root.join("unrelated.py"), "# padding\n".repeat(4000)).unwrap();
    f.build();
    let text = f.okay(&["brief", "credit", "--budget", "4096"]);
    assert!(text.contains("graph python: PARTIAL"));
    assert!(text.contains("return refund(amount)"), "{text}");
    assert!(text.contains("BODY_SENTINEL") && text.contains("not_full:"));
    fs::write(f.root.join("app.py"), "def current(): return 42\n").unwrap();
    let text = f.okay(&["brief", "credit", "--budget", "4096"]);
    assert!(text.contains("lexical selection only"));
    assert!(!text.contains("return refund(amount)"));
    assert!(text.contains("return 42"));
}
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture {
    root: PathBuf,
    language: &'static str,
    config: &'static str,
    source: &'static str,
}
impl Fixture {
    fn new(language: &'static str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "ae-popular-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let (config,source,files):(&str,&str,Vec<(&str,&str)>)=match language {
   "python"=>("pyproject.toml","ledger.py",vec![
    ("pyproject.toml","[project]\nname='semantic-fixture'\nversion='0.1.0'\n"),
    ("money.py","class Money:\n    cents: int\n"),
    ("ledger.py","# 雪 😀\r\nfrom money import Money\r\ndef credit(amount: Money) -> Money:\r\n    # BODY_SENTINEL\r\n    return amount\r\n"),
    ("app.py","from money import Money\nfrom ledger import credit as refund\ndef run(amount: Money) -> Money:\n    return refund(amount)\n")]),
   "rust"=>("Cargo.toml","src/ledger.rs",vec![
    ("Cargo.toml","[package]\nname='semantic_fixture'\nversion='0.1.0'\nedition='2021'\n"),
    ("src/lib.rs","pub mod money; pub mod ledger; pub mod app;\n"),
    ("src/money.rs","pub struct Money { pub cents: i32 }\n"),
    ("src/ledger.rs","// 雪 😀\r\nuse crate::money::Money;\r\npub fn credit(amount: Money) -> Money {\r\n    // BODY_SENTINEL\r\n    amount\r\n}\r\n"),
    ("src/app.rs","use crate::money::Money;\nuse crate::ledger::credit as refund;\npub fn run(amount: Money) -> Money { refund(amount) }\n")]),
   "typescript"=>("tsconfig.json","ledger.ts",vec![
    ("tsconfig.json","{\"compilerOptions\":{\"strict\":true,\"target\":\"ES2022\",\"module\":\"CommonJS\"},\"include\":[\"*.ts\"]}"),
    ("money.ts","export interface Money { cents: number; }\n"),
    ("ledger.ts","// 雪 😀\r\nimport { Money } from './money';\r\nexport function credit(amount: Money): Money {\r\n    // BODY_SENTINEL\r\n    return amount;\r\n}\r\n"),
    ("app.ts","import { Money } from './money';\nimport { credit as refund } from './ledger';\nexport function run(amount: Money): Money { return refund(amount); }\n")]),
   "javascript"=>("jsconfig.json","ledger.js",vec![
    ("jsconfig.json","{\"compilerOptions\":{\"allowJs\":true,\"checkJs\":true,\"target\":\"ES2022\",\"module\":\"CommonJS\"},\"include\":[\"*.js\"]}"),
    ("money.js","export class Money { /** @param {number} cents */ constructor(cents) { this.cents = cents; } }\n"),
    ("ledger.js","// 雪 😀\r\n/** @param {import('./money.js').Money} amount @returns {import('./money.js').Money} */\r\nexport function credit(amount) {\r\n    // BODY_SENTINEL\r\n    return amount;\r\n}\r\n"),
    ("app.js","import { credit as refund } from './ledger.js';\n/** @param {import('./money.js').Money} amount @returns {import('./money.js').Money} */\nexport function run(amount) { return refund(amount); }\n")]),
   _=>panic!("unsupported fixture")
  };
        for (path, text) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let f = Self {
            root,
            language,
            config,
            source,
        };
        f.okay(&["init"]);
        f
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_aienv"))
            .args(["--root", self.root.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap()
    }
    fn okay(&self, args: &[&str]) -> String {
        let r = self.run(args);
        assert!(
            r.status.success(),
            "{} {args:?}: {}",
            self.language,
            String::from_utf8_lossy(&r.stderr)
        );
        String::from_utf8(r.stdout).unwrap()
    }
    fn build(&self) -> Value {
        self.okay(&[
            "graph",
            "build",
            "--language",
            self.language,
            "--project",
            self.config,
        ]);
        serde_json::from_slice(
            &fs::read(
                self.root
                    .join(format!(".ai-env/semantic/{}.json", self.language)),
            )
            .unwrap(),
        )
        .unwrap()
    }
    fn status(&self) -> Value {
        serde_json::from_str(&self.okay(&["graph", "status", "--language", self.language])).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn named<'a>(graph: &'a Value, name: &str, kind: &str) -> &'a Value {
    let nodes: Vec<_> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["name"] == name && n["kind"] == kind && n["external"] == false)
        .collect();
    assert_eq!(
        nodes.len(),
        1,
        "Expected unambiguous {name}/{kind}, got {nodes:?}"
    );
    nodes[0]
}
fn reaches_type(graph: &Value, from: &str, to: &str, depth: usize) -> bool {
    if from == to {
        return true;
    }
    if depth == 0 {
        return false;
    }
    graph["edges"].as_array().unwrap().iter().any(|e| {
        e["source"] == from
            && matches!(
                e["kind"].as_str(),
                Some("uses_type" | "aliases" | "inherits")
            )
            && reaches_type(graph, e["target"].as_str().unwrap(), to, depth - 1)
    })
}
fn contracts(language: &'static str) {
    let f = Fixture::new(language);
    let cache = f.build();
    let graph = &cache["graph"];
    let caller = named(graph, "run", "function");
    let callee = named(graph, "credit", "function");
    let money = named(graph, "Money", "type");
    let source = caller["id"].as_str().unwrap();
    let target = callee["id"].as_str().unwrap();
    assert!(
        graph["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "calls" && e["source"] == source && e["target"] == target),
        "{language}: expected exact run -> credit call, graph={graph}"
    );
    assert!(
        reaches_type(graph, source, money["id"].as_str().unwrap(), 3),
        "{language}: typed run must depend on Money"
    );
    let contract = f.okay(&[
        "context",
        "--symbol",
        target,
        "--relations",
        "calls,callers,types",
        "--budget",
        "12000",
    ]);
    assert!(
        contract.contains("run")
            && contract.contains("Money")
            && !contract.contains("BODY_SENTINEL")
    );
    let body = f.okay(&["context", "--symbol", target, "--body", "--budget", "12000"]);
    assert!(body.contains("BODY_SENTINEL"));
    let impact = f.okay(&[
        "graph", "impact", target, "--depth", "2", "--budget", "12000",
    ]);
    assert!(impact.contains("run"));
    let path = callee["path"].as_str().unwrap();
    let bytes = fs::read(f.root.join(path)).unwrap();
    let start = callee["start_byte"].as_u64().unwrap() as usize;
    let end = callee["end_byte"].as_u64().unwrap() as usize;
    assert!(
        std::str::from_utf8(&bytes[start..end])
            .unwrap()
            .contains("BODY_SENTINEL"),
        "{language}: exact UTF8 compiler/body extent"
    );
    if language == "python" {
        assert_eq!(graph["analysis_complete"], false);
        assert!(contract.contains("PARTIAL"));
    }
}
fn freshness(language: &'static str) {
    let f = Fixture::new(language);
    let original = f.build();
    let original_id = named(&original["graph"], "credit", "function")["id"].clone();
    fs::write(
        f.root.join("README.md"),
        "An unrelated documentation edit\n",
    )
    .unwrap();
    assert_eq!(f.status()["fresh"], true, "{language}");
    let config = f.root.join(f.config);
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, format!("{text}\n")).unwrap();
    assert_eq!(
        f.status()["fresh"],
        false,
        "{language}: config change must stale graph"
    );
    assert!(!f
        .run(&["graph", "find", "credit", "--language", language])
        .status
        .success());
    f.okay(&["graph", "refresh", "--language", language]);
    assert_eq!(f.status()["fresh"], true);
    let source = f.root.join(f.source);
    let text = fs::read_to_string(&source).unwrap();
    fs::write(&source, text.replace("BODY_SENTINEL", "BODY_CHANGED")).unwrap();
    assert_eq!(f.status()["fresh"], false);
    f.okay(&["graph", "refresh", "--language", language]);
    let current: Value = serde_json::from_slice(
        &fs::read(f.root.join(format!(".ai-env/semantic/{language}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(
        named(&current["graph"], "credit", "function")["id"],
        original_id,
        "{language}: body-only change keeps canonical declaration identity"
    );
}
#[test]
fn python_contracts() {
    contracts("python");
}
#[test]
fn python_freshness() {
    freshness("python");
}
#[test]
fn rust_contracts() {
    contracts("rust");
}
#[test]
fn rust_freshness() {
    freshness("rust");
}
#[test]
fn typescript_contracts() {
    contracts("typescript");
}
#[test]
fn typescript_freshness() {
    freshness("typescript");
}
#[test]
fn javascript_contracts() {
    contracts("javascript");
}
#[test]
fn javascript_freshness() {
    freshness("javascript");
}
#[test]
fn graph_rejects_options_from_another_language_before_starting_a_backend() {
    for language in ["python", "rust", "typescript", "javascript"] {
        let f = Fixture::new(language);
        let r = f.run(&[
            "graph",
            "build",
            "--language",
            language,
            "--flag",
            "-Xclang",
        ]);
        assert!(!r.status.success());
        assert!(String::from_utf8_lossy(&r.stderr).contains("compiler-specific"));
    }
}

#[test]
fn analysis_does_not_execute_target_sitecustomize_from_pythonpath() {
    let f = Fixture::new("python");
    let marker = f.root.join("must-not-execute");
    let payload = format!(
        "from pathlib import Path\nPath({:?}).write_text('executed')\n",
        marker.to_string_lossy()
    );
    fs::write(f.root.join("sitecustomize.py"), payload).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_aienv"))
        .args([
            "--root",
            f.root.to_str().unwrap(),
            "graph",
            "build",
            "--language",
            "python",
        ])
        .env("PYTHONPATH", &f.root)
        .env("PYTHONSTARTUP", f.root.join("sitecustomize.py"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!marker.exists(),"Graph analysis must not execute project sitecustomize via inherited Python startup settings");
}

#[test]
fn changing_an_explicit_runtime_override_stales_existing_semantic_context() {
    let f = Fixture::new("typescript");
    f.build();
    let cache = f.root.join(".ai-env/semantic/typescript.json");
    let previous = fs::read(&cache).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_aienv"))
        .args([
            "--root",
            f.root.to_str().unwrap(),
            "graph",
            "find",
            "credit",
            "--language",
            "typescript",
        ])
        .env("AI_ENV_NODE", f.root.join("different-node"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Stale semantic index"));
    assert_eq!(fs::read(cache).unwrap(), previous);
}

#[test]
fn overloaded_implementation_impact_and_reference_navigation_reach_actual_callers() {
    let f = Fixture::new("typescript");
    fs::write(f.root.join("ledger.ts"),"import { Money } from './money';\nexport function credit(amount: Money): Money;\nexport function credit(amount: number): number;\nexport function credit(amount: Money | number): Money | number {\n // BODY_SENTINEL\n return amount;\n}\n").unwrap();
    fs::write(f.root.join("counts.ts"),"import { credit } from './ledger';\nexport function runCount(n:number):number {return credit(n);}\nexport function unrelated(n:number):number {return n;}\n").unwrap();
    let cache = f.build();
    let graph = &cache["graph"];
    let variants: Vec<_> = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["name"] == "credit" && n["kind"] == "function")
        .collect();
    assert_eq!(variants.len(), 3);
    let implementation = *variants.iter().find(|n| n["definition"] == true).unwrap();
    let contract = *variants.iter().find(|n| n["definition"] == false).unwrap();
    let implementation_id = implementation["id"].as_str().unwrap();
    let impact = f.okay(&[
        "graph",
        "impact",
        implementation_id,
        "--depth",
        "2",
        "--budget",
        "12000",
    ]);
    for caller in ["run", "runCount"] {
        let id = named(graph, caller, "function")["id"].as_str().unwrap();
        assert!(
            impact.contains(&aienv::graph::model::public_id("typescript", id)),
            "Missing {caller} in implementation impact"
        );
    }
    let unrelated = named(graph, "unrelated", "function")["id"]
        .as_str()
        .unwrap();
    assert!(!impact.contains(&aienv::graph::model::public_id("typescript", unrelated)));
    let references = f.okay(&[
        "context",
        "--symbol",
        contract["id"].as_str().unwrap(),
        "--relations",
        "references",
        "--budget",
        "12000",
    ]);
    assert!(references.contains(&aienv::graph::model::public_id(
        "typescript",
        implementation_id
    )));
    let body = f.okay(&[
        "context",
        "--symbol",
        implementation_id,
        "--body",
        "--budget",
        "12000",
    ]);
    assert!(body.contains("BODY_SENTINEL"));
}
