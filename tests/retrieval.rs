use aienv::{project::Project, retrieval};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "ae-retrieval-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn query(&self, task: &[u8], precise: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aienv"));
        command.args(["--root", self.0.to_str().unwrap(), "query"]);
        if let Some(value) = precise {
            command.args(["--query", value]);
        }
        run_input(command, task)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn run_input(mut command: Command, input: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn stateless_cli_uses_the_native_projector_without_initializing_a_repository() {
    let fixture = Fixture::new();
    let task = format!(
        "Keep early_owner. {} Late: stock_ceiling, check_regressions, КвотаРезерва.",
        "background detail ".repeat(350)
    );
    for precise in [
        None,
        Some("  Quote::total  src/pricing.rs  "),
        Some("--root literal"),
    ] {
        let output = fixture.query(task.as_bytes(), precise);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["query"], retrieval::project(&task, precise).unwrap());
        assert_eq!(value["has_terms"], true);
        assert_eq!(
            value["method"],
            if precise.is_some() {
                "explicit"
            } else {
                "whole-task-lexical"
            }
        );
    }
    let output = fixture.query(b"--root literal", None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["query"], "--root literal");
    assert!(!fixture.0.join(".ai-env").exists());
    for task in [
        b"".to_vec(),
        vec![b'x'; 65537],
        vec![0xff],
        b"task\0symbol".to_vec(),
    ] {
        let output = fixture.query(&task, None);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    for precise in ["", "x\ny", &"я".repeat(1025)] {
        let output = fixture.query(b"task", Some(precise));
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}

#[cfg(unix)]
#[test]
fn real_python_host_uses_identical_rust_query_and_preserves_the_full_task() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    Project::init(&fixture.0).unwrap();
    fs::write(fixture.0.join("app.py"), "def quote(): return 17\n").unwrap();
    let capture = fixture.0.join("capture.txt");
    let wrapper = fixture.0.join("capture-aienv");
    fs::write(&wrapper, "#!/bin/sh\nif [ \"$3\" = brief ]; then\n  printf '%s\\n' \"$@\" > \"$AI_ENV_TEST_CAPTURE\"\nfi\nexec \"$AI_ENV_TEST_REAL\" \"$@\"\n").unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let task = format!(
        "Keep early_owner. {} Enforce stock_ceiling; run check_regressions and КвотаРезерва.",
        "ordinary background context ".repeat(240)
    );
    for precise in [
        None,
        Some("  Quote::total  src/pricing.rs  "),
        Some("--root literal"),
    ] {
        let mut command = Command::new("python3");
        command
            .args([
                "-I",
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/skills/ai-env/scripts/prepare_task.py"
                ),
                "--root",
                fixture.0.to_str().unwrap(),
                "--task",
                "-",
            ])
            .env("AI_ENV_BIN", &wrapper)
            .env("AI_ENV_TEST_REAL", env!("CARGO_BIN_EXE_aienv"))
            .env("AI_ENV_TEST_CAPTURE", &capture);
        if let Some(query) = precise {
            command.arg(format!("--query={query}"));
        }
        let output = run_input(command, task.as_bytes());
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).starts_with(&task));
        let arguments = fs::read_to_string(&capture).unwrap();
        let arguments: Vec<_> = arguments.lines().collect();
        let impact = arguments.iter().position(|a| *a == "--impact").unwrap();
        let terminator = arguments.iter().position(|a| *a == "--").unwrap();
        assert!(terminator > impact);
        assert_eq!(
            arguments[terminator + 1],
            retrieval::project(&task, precise).unwrap()
        );
    }
    for task in ["a", "я", "--root literal"] {
        let mut command = Command::new("python3");
        command
            .args([
                "-I",
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/skills/ai-env/scripts/prepare_task.py"
                ),
                "--root",
                fixture.0.to_str().unwrap(),
                "--task",
                "-",
            ])
            .env("AI_ENV_BIN", &wrapper)
            .env("AI_ENV_TEST_REAL", env!("CARGO_BIN_EXE_aienv"))
            .env("AI_ENV_TEST_CAPTURE", &capture);
        let output = run_input(command, task.as_bytes());
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let arguments = fs::read_to_string(&capture).unwrap();
        assert_eq!(arguments.lines().any(|arg| arg == "--impact"), task != "a");
    }
    for precise in ["", "bad\nquery", &"x".repeat(2049)] {
        let mut command = Command::new("python3");
        command
            .args([
                "-I",
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/skills/ai-env/scripts/prepare_task.py"
                ),
                "--root",
                fixture.0.to_str().unwrap(),
                "--task",
                "-",
                "--query",
                precise,
            ])
            .env("AI_ENV_BIN", env!("CARGO_BIN_EXE_aienv"));
        let output = run_input(command, b"Task is retained only on success");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}
