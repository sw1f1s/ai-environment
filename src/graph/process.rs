//! Bounded stdio helper execution; project code is never selected as the helper.
use crate::Result;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
const MAX_OUTPUT: usize = 16 * 1024 * 1024;
fn kill(pid: u32) {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        unsafe {
            kill(-(pid as i32), 9);
        }
    }
}
pub fn invoke(script: &Path, payload: Vec<u8>, cwd: &Path) -> Result<Vec<u8>> {
    invoke_until(
        script,
        payload,
        cwd,
        Instant::now() + Duration::from_secs(120),
    )
}
pub(crate) fn invoke_until(
    script: &Path,
    payload: Vec<u8>,
    cwd: &Path,
    deadline: Instant,
) -> Result<Vec<u8>> {
    let timeout = deadline.saturating_duration_since(Instant::now());
    if timeout.is_zero() {
        return Err("Semantic analysis deadline exhausted; no helper started".into());
    }
    if payload.len() > 192 * 1024 * 1024 {
        return Err("Analysis input too large; select a smaller project root".into());
    }
    let mut command = Command::new("python3");
    command
        .args(["-E", "-s", "-X", "utf8"])
        .arg(script)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // PYTHONPATH=. is common in application shells. It must not import target
    // sitecustomize or leak into Jedi's interpreter helper subprocesses.
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("PYTHON") {
            command.env_remove(name);
        }
    }
    command.env("PYTHONNOUSERSITE", "1");
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(not(unix))]
    return Err("Semantic helpers currently require macOS/Linux process groups".into());
    let mut child = command.spawn()?;
    let pid = child.id();
    let mut input = child.stdin.take().ok_or("Missing helper stdin")?;
    let (sent_tx, sent_rx) = mpsc::channel();
    let writer = thread::spawn(move || {
        let result = input.write_all(&payload);
        drop(input);
        let _ = sent_tx.send(result.is_ok());
    });
    let output = Arc::new(Mutex::new([Vec::new(), Vec::new()]));
    let bytes = Arc::new(AtomicUsize::new(0));
    let exceeded = Arc::new(AtomicBool::new(false));
    let (done_tx, done_rx) = mpsc::channel();
    let streams: [Box<dyn Read + Send>; 2] = [
        Box::new(child.stdout.take().ok_or("Missing stdout")?),
        Box::new(child.stderr.take().ok_or("Missing stderr")?),
    ];
    let mut readers = Vec::new();
    for (index, mut stream) in streams.into_iter().enumerate() {
        let out = output.clone();
        let used = bytes.clone();
        let limit = exceeded.clone();
        let done = done_tx.clone();
        readers.push(thread::spawn(move || {
            let mut block = [0; 8192];
            let mut good = true;
            loop {
                match stream.read(&mut block) {
                    Ok(0) => break,
                    Ok(n) => {
                        let offset = used.fetch_add(n, Ordering::SeqCst);
                        let take = n.min(MAX_OUTPUT.saturating_sub(offset));
                        if take > 0 {
                            out.lock().unwrap()[index].extend_from_slice(&block[..take]);
                        }
                        if take < n {
                            limit.store(true, Ordering::SeqCst);
                            kill(pid);
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
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
    let mut completed = 0;
    let mut complete = true;
    let mut timed_out = false;
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(value) => status = value,
                Err(e) => {
                    kill(pid);
                    let _ = child.wait();
                    return Err(e.into());
                }
            }
        }
        while let Ok(good) = done_rx.try_recv() {
            completed += 1;
            complete &= good;
        }
        if status.is_some() && completed == 2 {
            break;
        }
        if exceeded.load(Ordering::SeqCst) || Instant::now() >= deadline {
            timed_out = !exceeded.load(Ordering::SeqCst);
            kill(pid);
            let _ = child.kill();
            if status.is_none() {
                status = Some(child.wait()?);
            }
            let deadline = Instant::now() + Duration::from_millis(300);
            while completed < 2 && Instant::now() < deadline {
                if let Ok(good) = done_rx.recv_timeout(Duration::from_millis(10)) {
                    completed += 1;
                    complete &= good;
                }
            }
            break;
        }
        if completed == 2 {
            thread::sleep(Duration::from_millis(10));
        } else if let Ok(good) = done_rx.recv_timeout(Duration::from_millis(10)) {
            completed += 1;
            complete &= good;
        }
    }
    if writer.is_finished() {
        let _ = writer.join();
    }
    for reader in readers {
        if reader.is_finished() {
            let _ = reader.join();
        }
    }
    let sent = sent_rx
        .recv_timeout(Duration::from_millis(250))
        .unwrap_or(false);
    let out = output.lock().map_err(|_| "Helper capture lock failed")?;
    let error = String::from_utf8_lossy(&out[1]);
    let excerpt: String = if error.trim().is_empty() {
        // Helpers can return structured failure diagnostics on stdout before
        // exiting nonzero. Preserve the actionable cause without publishing a graph.
        serde_json::from_slice::<serde_json::Value>(&out[0])
            .ok()
            .and_then(|value| {
                value.get("diagnostics")?.as_array().map(|items| {
                    items
                        .iter()
                        .filter_map(|d| d.get("message")?.as_str())
                        .take(5)
                        .collect::<Vec<_>>()
                        .join("; ")
                        .chars()
                        .take(3000)
                        .collect::<String>()
                })
            })
            .unwrap_or_default()
    } else {
        error.chars().take(3000).collect()
    };
    if timed_out {
        return Err(format!(
            "Semantic analysis timed out after {:.1} seconds. Narrow selected files. {excerpt}",
            timeout.as_secs_f64()
        )
        .into());
    }
    if exceeded.load(Ordering::SeqCst) {
        return Err("Analyzer output exceeded 16 MiB; narrow selected files".into());
    }
    if !status.is_some_and(|s| s.success()) || !complete || completed != 2 || !sent {
        return Err(format!("Semantic analyzer failed; no graph published. {excerpt}").into());
    }
    Ok(out[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nonzero_helper_keeps_actionable_structured_diagnostics() {
        let root = std::env::temp_dir().join(format!(
            "aienv-helper-error-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let script = root.join("adapter.py");
        std::fs::write(&script,"import json,sys\njson.load(sys.stdin)\nprint(json.dumps({'diagnostics':[{'message':'Required compiler runtime unavailable; set its explicit path'}]}))\nsys.exit(1)\n").unwrap();
        let error = invoke(&script, b"{}".to_vec(), &root)
            .unwrap_err()
            .to_string();
        std::fs::remove_dir_all(root).unwrap();
        assert!(error.contains("Required compiler runtime unavailable; set its explicit path"));
        assert!(error.contains("no graph published"));
    }

    #[test]
    fn bounded_helper_timeout_kills_the_process_group_and_expired_deadline_never_starts() {
        let root = std::env::temp_dir().join(format!(
            "aienv-helper-timeout-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let script = root.join("adapter.py");
        // The child inherits stdout. Returning promptly requires killing the
        // process group even though the helper itself exits successfully.
        std::fs::write(&script,
            "import json,subprocess,sys\njson.load(sys.stdin)\nsubprocess.Popen([sys.executable,'-c','import time; time.sleep(30)'])\nprint('{}')\n"
        ).unwrap();
        let started = Instant::now();
        let error = invoke_until(
            &script,
            b"{}".to_vec(),
            &root,
            started + Duration::from_millis(150),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
        std::fs::write(&script, "open('started', 'w').write('unsafe')\n").unwrap();
        let error = invoke_until(&script, b"{}".to_vec(), &root, Instant::now())
            .unwrap_err()
            .to_string();
        assert!(error.contains("no helper started"), "{error}");
        assert!(!root.join("started").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
