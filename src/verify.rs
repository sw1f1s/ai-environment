//! Fixed, user-configured checks. Process groups bound ordinary child processes;
//! this is not a sandbox for commands selected by the repository owner.
use crate::{
    json,
    project::{sha, valid_id, Project},
    Result,
};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    fs,
    io::Read,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const OUTPUT_LIMIT: usize = 65_536;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub id: String,
    pub snapshot: String,
    pub check: String,
    pub argv: Vec<String>,
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub output_limited: bool,
    #[serde(default)]
    pub config_hash: String,
    #[serde(default)]
    pub source_unchanged: bool,
    #[serde(default)]
    pub elapsed_millis: u64,
    #[serde(default = "default_true")]
    pub capture_complete: bool,
    #[serde(default)]
    pub snapshot_after: Option<String>,
    #[serde(default)]
    pub execution_error: Option<String>,
}

pub fn load(project: &Project, id: &str) -> Result<Proof> {
    if !valid_id(id) {
        return Err("Invalid proof ID".into());
    }
    // Option distinguishes a missing proof from malformed/default success data.
    let proof: Option<Proof> = project.read_state(&format!("proofs/{id}.json"))?;
    let proof = proof.ok_or("Proof does not exist")?;
    if proof.id != id {
        return Err("Proof ID does not match its file".into());
    }
    Ok(proof)
}

fn terminate_group(pid: u32) {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // The process was placed in its own process group before exec.
        unsafe {
            kill(-(pid as i32), 9);
        }
    }
}

#[derive(Debug)]
struct Capture {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
    output_limited: bool,
    complete: bool,
    elapsed_millis: u64,
}

fn bounded_text(bytes: &[u8], limit: usize) -> (String, bool) {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    let truncated = text.len() > limit;
    if truncated {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    (text, truncated)
}

fn capture(project: &Project, argv: &[String], timeout: Duration) -> Result<Capture> {
    let started = Instant::now();
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(&project.root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(not(unix))]
    return Err("Checks require macOS/Linux process groups".into());
    let mut child = command.spawn()?;
    let pid = child.id();
    let output = Arc::new(Mutex::new([Vec::new(), Vec::new()]));
    let used = Arc::new(AtomicUsize::new(0));
    let limited = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = mpsc::channel();
    let streams: [Box<dyn Read + Send>; 2] = [
        Box::new(child.stdout.take().ok_or("Missing stdout pipe")?),
        Box::new(child.stderr.take().ok_or("Missing stderr pipe")?),
    ];
    let mut readers = Vec::new();
    for (index, mut stream) in streams.into_iter().enumerate() {
        let output = output.clone();
        let used = used.clone();
        let limited = limited.clone();
        let done = done_tx.clone();
        readers.push(thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            let mut good = true;
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let offset = used.fetch_add(count, Ordering::SeqCst);
                        let retained = count.min(OUTPUT_LIMIT.saturating_sub(offset));
                        if retained > 0 {
                            output.lock().unwrap()[index].extend_from_slice(&buffer[..retained]);
                        }
                        if count > retained {
                            limited.store(true, Ordering::SeqCst);
                            terminate_group(pid);
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        good = false;
                        break;
                    }
                }
            }
            let _ = done.send(good);
        }));
    }
    drop(done_tx);
    let mut status = None;
    let mut finished = 0;
    let mut capture_good = true;
    let mut timed_out = false;
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(value) => status = value,
                Err(error) => {
                    terminate_group(pid);
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error.into());
                }
            }
        }
        while let Ok(good) = done_rx.try_recv() {
            finished += 1;
            capture_good &= good;
        }
        if status.is_some() && finished == 2 {
            // A check must not leave background group members running after exit.
            terminate_group(pid);
            break;
        }
        if limited.load(Ordering::SeqCst) || started.elapsed() >= timeout {
            timed_out = !limited.load(Ordering::SeqCst) && started.elapsed() >= timeout;
            terminate_group(pid);
            let _ = child.kill();
            if status.is_none() {
                status = Some(child.wait()?);
            }
            // A command that deliberately escapes its group is outside this local
            // permission boundary; inherited pipes still must not hang the CLI.
            let deadline = Instant::now() + Duration::from_millis(250);
            while finished < 2 && Instant::now() < deadline {
                if let Ok(good) = done_rx.recv_timeout(Duration::from_millis(10)) {
                    finished += 1;
                    capture_good &= good;
                }
            }
            break;
        }
        if finished == 2 {
            // Commands may close both output streams while continuing work.
            thread::sleep(Duration::from_millis(10));
        } else {
            match done_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(good) => {
                    finished += 1;
                    capture_good &= good;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    finished = 2;
                    capture_good = false;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
    // Join completed readers only; dropping an unfinished handle detaches it.
    for reader in readers {
        if reader.is_finished() {
            let _ = reader.join();
        }
    }
    let captured = output.lock().map_err(|_| "Output capture lock failed")?;
    let (stdout, first_truncated) = bounded_text(&captured[0], OUTPUT_LIMIT);
    let (stderr, second_truncated) = bounded_text(&captured[1], OUTPUT_LIMIT - stdout.len());
    Ok(Capture {
        exit_code: status.and_then(|s| s.code()),
        stdout,
        stderr,
        timed_out,
        output_limited: limited.load(Ordering::SeqCst) || first_truncated || second_truncated,
        complete: finished == 2 && capture_good,
        elapsed_millis: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    })
}

pub fn run(project: &Project, args: &[String]) -> Result<String> {
    if args.len() != 1 || !valid_id(&args[0]) {
        return Err("Usage: aienv verify CHECK_NAME".into());
    }
    let before = project.snapshot()?;
    let config = project.config()?;
    let check = config
        .checks
        .get(&args[0])
        .ok_or("Unknown configured check; add a named argv check to .ai-env/config.json")?;
    if check.argv.is_empty()
        || check.argv[0].is_empty()
        || check.argv.iter().any(|s| s.contains('\0'))
        || !(1..=600).contains(&check.timeout_seconds)
    {
        return Err("Check requires nonempty argv and timeout_seconds between 1 and 600".into());
    }
    let config_hash = sha(fs::read(project.state.join("config.json"))?);
    if project.snapshot()?.hash != before.hash {
        return Err("Source/config changed while preparing check; retry verification".into());
    }
    let identity = format!(
        "{}:{}:{}:{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        SEQUENCE.fetch_add(1, Ordering::SeqCst),
        check.argv.join("\0")
    );
    let id = format!("p{}", &sha(identity)[..32]);
    let execution = capture(
        project,
        &check.argv,
        Duration::from_secs(check.timeout_seconds),
    );
    let after = project.snapshot();
    let snapshot_after = after.as_ref().ok().map(|s| s.hash.clone());
    let unchanged = snapshot_after.as_ref() == Some(&before.hash);
    let mut proof = Proof {
        id,
        snapshot: before.hash,
        check: args[0].clone(),
        argv: check.argv.clone(),
        passed: false,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        output_limited: false,
        config_hash,
        source_unchanged: unchanged,
        elapsed_millis: 0,
        capture_complete: false,
        snapshot_after,
        execution_error: None,
    };
    match execution {
        Ok(value) => {
            proof.exit_code = value.exit_code;
            proof.stdout = value.stdout;
            proof.stderr = value.stderr;
            proof.timed_out = value.timed_out;
            proof.output_limited = value.output_limited;
            proof.capture_complete = value.complete;
            proof.elapsed_millis = value.elapsed_millis;
        }
        Err(error) => proof.execution_error = Some(error.to_string()),
    }
    if let Err(error) = after {
        proof.execution_error = Some(format!("Cannot read snapshot after check: {error}"));
    }
    proof.passed = proof.exit_code == Some(0)
        && !proof.timed_out
        && !proof.output_limited
        && proof.capture_complete
        && proof.source_unchanged
        && proof.execution_error.is_none();
    project.write_state(&format!("proofs/{}.json", proof.id), &proof)?;
    json(&proof)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Check, Config};
    use std::{collections::BTreeMap, path::PathBuf};
    struct Fixture {
        project: Project,
        path: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "aienv-check-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir(&path).unwrap();
            let project = Project::init(&path).unwrap();
            fs::write(path.join("main.py"), "print(1)\n").unwrap();
            Self { project, path }
        }
        fn check(&self, argv: &[&str], timeout: u64) {
            let mut checks = BTreeMap::new();
            checks.insert(
                "qa".into(),
                Check {
                    argv: argv.iter().map(|s| s.to_string()).collect(),
                    timeout_seconds: timeout,
                },
            );
            self.project
                .write_state("config.json", &Config { checks })
                .unwrap();
        }
        fn run(&self) -> Proof {
            serde_json::from_str(&run(&self.project, &["qa".into()]).unwrap()).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
    #[test]
    fn successful_proof_is_persisted_and_bound_to_snapshot() {
        let f = Fixture::new();
        f.check(&["/bin/sh", "-c", "printf checked"], 2);
        let p = f.run();
        assert!(p.passed);
        assert_eq!(p.stdout, "checked");
        assert_eq!(
            load(&f.project, &p.id).unwrap().snapshot,
            f.project.snapshot().unwrap().hash
        );
        assert_ne!(p.id, f.run().id);
    }
    #[test]
    fn nonzero_and_spawn_failure_still_produce_failed_proofs() {
        let f = Fixture::new();
        f.check(&["/bin/sh", "-c", "printf failure >&2; exit 7"], 2);
        let p = f.run();
        assert!(!p.passed);
        assert_eq!(p.exit_code, Some(7));
        assert_eq!(p.stderr, "failure");
        f.check(&["/no/such/aienv-check"], 2);
        let p = f.run();
        assert!(!p.passed);
        assert!(p.execution_error.is_some());
    }
    #[test]
    fn changing_source_invalidates_check() {
        let f = Fixture::new();
        f.check(&["/bin/sh", "-c", "printf changed > main.py"], 2);
        let p = f.run();
        assert!(!p.passed);
        assert!(!p.source_unchanged);
        assert_eq!(p.exit_code, Some(0));
    }
    #[test]
    fn changing_config_during_check_invalidates_proof() {
        let f = Fixture::new();
        f.check(
            &[
                "/bin/sh",
                "-c",
                "printf '{\"checks\":{}}' > .ai-env/config.json",
            ],
            2,
        );
        let proof = f.run();
        assert_eq!(proof.exit_code, Some(0));
        assert!(!proof.passed);
        assert!(!proof.source_unchanged);
        assert_ne!(
            proof.config_hash,
            sha(fs::read(f.project.state.join("config.json")).unwrap())
        );
    }

    #[test]
    fn output_is_bounded_and_command_killed() {
        let f = Fixture::new();
        f.check(&["/usr/bin/yes"], 2);
        let p = f.run();
        assert!(!p.passed);
        assert!(p.output_limited);
        assert!(p.stdout.len() + p.stderr.len() <= OUTPUT_LIMIT);
    }
    #[test]
    fn timeout_kills_descendant_process_group() {
        let f = Fixture::new();
        f.check(
            &["/bin/sh", "-c", "sleep 2; printf escaped > descendant.txt"],
            1,
        );
        let started = Instant::now();
        let p = f.run();
        assert!(p.timed_out);
        assert!(!p.passed);
        assert!(started.elapsed() < Duration::from_secs(2));
        thread::sleep(Duration::from_millis(1200));
        assert!(!f.path.join("descendant.txt").exists());
    }
    #[test]
    fn invalid_utf8_and_multibyte_output_still_obey_encoded_budget() {
        let (text, limited) = bounded_text(&vec![255; 40_000], 65_536);
        assert!(limited);
        assert!(text.len() <= 65_536);
        assert!(text.chars().all(|c| c == '\u{fffd}'));
        let (text, limited) = bounded_text("😀漢".as_bytes(), 5);
        assert!(limited);
        assert_eq!(text, "😀");
    }

    #[test]
    fn missing_invalid_and_unconfigured_checks_reject() {
        let f = Fixture::new();
        assert!(run(&f.project, &[]).is_err());
        assert!(run(&f.project, &["qa".into()]).is_err());
        assert!(load(&f.project, "../outside").is_err());
        f.check(&["/bin/echo", "hi"], 0);
        assert!(run(&f.project, &["qa".into()]).is_err());
    }
}
