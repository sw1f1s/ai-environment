use aienv::{profile, project::Project};
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ae-profile-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Project::init(&path).unwrap();
        Self(path)
    }
    fn write(&self, path: &str, text: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn read(&self, args: &[&str]) -> (String, Value) {
        let text = profile::run(
            &Project::open(&self.0).unwrap(),
            &args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>(),
        )
        .unwrap();
        let value = serde_json::from_str(&text).unwrap();
        (text, value)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn test_discovery_retains_popular_language_evidence_without_inventing_a_runner() {
    let f = Fixture::new();
    f.write(
        "go/value_test.go",
        "package value\nimport \"testing\"\nfunc TestValue(t *testing.T) {}\n",
    );
    f.write(
        "java/ValueTest.java",
        "import org.junit.Test; class ValueTest { @Test void value() {} }\n",
    );
    f.write(
        "kotlin/ValueTests.kt",
        "import kotlin.test.Test\n@Test fun value() {}\n",
    );
    f.write(
        "swift/ValueTests.swift",
        "import XCTest\nfinal class ValueTests: XCTestCase {}\n",
    );
    let (_, v) = f.read(&[]);
    assert_eq!(v["coverage"]["tests_found"], 4);
    assert_eq!(v["testing"]["unconfigured"], true);
    assert!(v["testing"]["configured"].as_array().unwrap().is_empty());
    assert!(v["testing"]["candidates"].as_array().unwrap().is_empty());
}

#[test]
fn scoped_explicit_configs_remain_distinct_from_inferred_mixed_source() {
    let f = Fixture::new();
    f.write(
        ".editorconfig",
        "root = true\n[*]\nindent_style = space\nindent_size = 4\n",
    );
    f.write("src/.editorconfig", "[*.py]\nindent_size = 2\n");
    f.write("other/.clang-format", "Unrelated: true\n");
    f.write("src/a.py", "def first():\n  return 1\n");
    f.write("src/b.py", "def second():\r\n\treturn 2\r\n");
    let (text, v) = f.read(&["--path", "src"]);
    let configs = v["style"]["configs"].as_array().unwrap();
    assert_eq!(configs.len(), 2);
    assert_eq!(configs[0]["scope"], ".");
    assert_eq!(configs[1]["scope"], "src");
    assert!(!text.contains("Unrelated"));
    let observation = &v["style"]["observations"][0];
    assert_eq!(observation["inferred"], true);
    assert_eq!(observation["indentation"], "mixed_widths_or_nesting");
    assert_eq!(observation["newline"], "mixed");
    assert!(v["style"]["precedence"]
        .as_str()
        .unwrap()
        .contains("Explicit repository rules override"));
    assert_eq!(text, f.read(&["--path", "src"]).0);
}

#[test]
fn tests_outside_requested_source_scope_and_configured_checks_are_discovered() {
    let f = Fixture::new();
    f.write(
        "src/lib.rs",
        "#[test]\nfn checks_rule() { assert!(true); }\n",
    );
    f.write("Cargo.toml", "[package]\nname='fixture'\nversion='0.1.0'\n");
    f.write("tests/orders.rs", "#[test]\nfn integration() {}\n");
    f.write(
        "tests/integration/test_api.py",
        "import unittest\nclass Tests(unittest.TestCase): pass\n",
    );
    f.write("spec/price.spec.ts", "describe('price', () => {});\n");
    f.write(
        "test/Checks.csproj",
        "<PackageReference Include=\"xunit\" />",
    );
    f.write("test/Checks.cs", "[Fact]\npublic void Works() {}\n");
    f.write("cpp/order_test.cpp", "TEST(Order, Good) {}\n");
    f.write(
        "cpp/CMakeLists.txt",
        "enable_testing()\nadd_test(NAME Orders COMMAND order_tests)\n",
    );
    f.write("package.json", r#"{"scripts":{"test":"node --test"}}"#);
    f.write(
        ".ai-env/config.json",
        r#"{"checks":{"suite":{"argv":["python3","check.py"],"timeout_seconds":37}}}"#,
    );
    let (_, v) = f.read(&["--path", "src", "--budget", "24000"]);
    assert_eq!(v["testing"]["configured"][0]["timeout_seconds"], 37);
    assert_eq!(v["testing"]["unconfigured"], false);
    let tests = v["testing"]["discovered"].as_array().unwrap();
    assert!(tests
        .iter()
        .any(|test| test["path"] == "src/lib.rs" && test["kind"] == "unit"));
    assert!(tests.iter().any(|test| test["path"] == "tests/orders.rs"
        && test["kind"] == "integration"
        && test["in_requested_scope"] == false));
    assert!(tests.iter().any(
        |test| test["path"] == "tests/integration/test_api.py" && test["kind"] == "integration"
    ));
    assert!(tests
        .iter()
        .any(|test| test["path"] == "spec/price.spec.ts"));
    assert!(tests
        .iter()
        .any(|test| test["path"] == "cpp/order_test.cpp"));
    let candidates = v["testing"]["candidates"].as_array().unwrap();
    assert!(tests
        .iter()
        .any(|test| test["path"] == "cpp/CMakeLists.txt" && test["kind"] == "unknown"));
    assert!(candidates
        .iter()
        .any(|c| c["argv"][0] == "cargo" && c["kind"] == "unit"));
    assert!(candidates
        .iter()
        .any(|c| c["argv"][0] == "dotnet" && c["argv"][3] == "--no-restore"));
    assert!(candidates.iter().any(|c| c["argv"][0] == "npm"));
    assert!(candidates.iter().any(|c| c["argv"][0] == "python3"));
}

#[test]
fn discovery_does_not_execute_configs_or_test_modules_and_refreshes_evidence() {
    let f = Fixture::new();
    f.write(
        "eslint.config.js",
        "require('fs').writeFileSync('EXECUTED','bad');\n",
    );
    f.write(
        "tests/test_side_effect.py",
        "open('EXECUTED', 'w').write('bad')\n",
    );
    let (_, before) = f.read(&[]);
    assert!(!f.0.join("EXECUTED").exists());
    f.write("tests/test_side_effect.py", "# changed\n");
    let (_, after) = f.read(&[]);
    assert_ne!(before["snapshot"], after["snapshot"]);
    assert_ne!(
        before["testing"]["discovered"][0]["hash"],
        after["testing"]["discovered"][0]["hash"]
    );
    assert!(!f.0.join("EXECUTED").exists());
}

#[test]
fn bounded_unicode_packet_discloses_omissions_without_claiming_success() {
    let f = Fixture::new();
    f.write(".editorconfig", &"# 雪😀\n".repeat(1000));
    for index in 0..90 {
        f.write(&format!("tests/test_{index:03}.py"), "# candidate\n");
    }
    let (text, v) = f.read(&["--budget", "2048"]);
    assert!(text.len() <= 2048);
    assert_eq!(v["coverage"]["tests_found"], 90);
    assert!(v["coverage"]["omitted"]["discovered"].as_u64().unwrap() > 0);
    assert!(
        v["coverage"]["config_content_omitted_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(v["coverage"]["complete"], false);
    assert_eq!(v["testing"]["unconfigured"], true);
    assert!(v["testing"]["status"]
        .as_str()
        .unwrap()
        .contains("not execution"));
}

#[test]
fn manifest_style_selection_avoids_unrelated_dependency_payloads() {
    let f = Fixture::new();
    f.write("pyproject.toml", "[project]\ndependencies = ['UNRELATED_DEPENDENCY_SENTINEL']\n[tool.ruff]\nline-length = 88\n[tool.ruff.format]\nquote-style = 'single'\n[tool.other]\nvalue = 'UNRELATED_TOOL_SENTINEL'\n");
    f.write("package.json",r#"{"dependencies":{"UNRELATED_PACKAGE_SENTINEL":"1"},"prettier":{"singleQuote":true},"scripts":{"test":"node --test"}}"#);
    let (text, v) = f.read(&[]);
    assert!(text.contains("line-length = 88") && text.contains("singleQuote"));
    assert!(
        !text.contains("UNRELATED_DEPENDENCY_SENTINEL")
            && !text.contains("UNRELATED_TOOL_SENTINEL")
            && !text.contains("UNRELATED_PACKAGE_SENTINEL")
    );
    assert_eq!(v["style"]["configs"].as_array().unwrap().len(), 2);
}

#[test]
fn empty_project_and_invalid_arguments_are_explicit() {
    let f = Fixture::new();
    let (_, v) = f.read(&[]);
    assert_eq!(v["coverage"]["tests_found"], 0);
    assert_eq!(v["testing"]["unconfigured"], true);
    assert_eq!(v["testing"]["candidates"], serde_json::json!([]));
    let p = Project::open(&f.0).unwrap();
    for args in [
        vec!["--path", "../secret"],
        vec!["--path", ".env"],
        vec!["--budget", "1"],
        vec!["--budget", "2048", "--budget", "4096"],
        vec!["--run"],
    ] {
        assert!(profile::run(&p, &args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).is_err());
    }
}

#[cfg(unix)]
#[test]
fn linked_and_ignored_config_inputs_are_not_read() {
    let f = Fixture::new();
    f.write("outside.txt", "PRIVATE_SENTINEL\n");
    std::os::unix::fs::symlink(f.0.join("outside.txt"), f.0.join(".editorconfig")).unwrap();
    let (text, v) = f.read(&[]);
    assert!(!text.contains("PRIVATE_SENTINEL"));
    assert_eq!(v["style"]["configs"], serde_json::json!([]));
    assert!(v["coverage"]["snapshot_excluded"].as_u64().unwrap() > 0);
}
