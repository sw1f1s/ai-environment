//! Source-only CAS transactions with ordinary-I/O rollback, not power-loss atomicity.
use crate::{
    json,
    project::{sha, Project, Snapshot, MAX_FILE},
    tasks, Result,
};
use serde::Deserialize;
use serde_json::{json as value, Value};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions, Permissions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_PATCH: u64 = 16 * 1024 * 1024;
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Patch {
    snapshot: String,
    edits: Vec<Edit>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    path: String,
    expected_hash: Value,
    source: Option<String>,
    old: Option<String>,
    new: Option<String>,
}
struct Prepared {
    relative: String,
    path: PathBuf,
    source: Vec<u8>,
    before: Option<Vec<u8>>,
    mode: Option<Permissions>,
}

fn exact_hash(expected: &Value, current: Option<&str>) -> bool {
    match (expected, current) {
        (Value::Null, None) => true,
        (Value::String(a), Some(b)) => a == b,
        _ => false,
    }
}
fn prepare(
    project: &Project,
    before: &Snapshot,
    patch: Patch,
    task: Option<&str>,
    owner: Option<&str>,
) -> Result<Vec<Prepared>> {
    if patch.snapshot != before.hash {
        return Err("Stale snapshot; refresh context before editing".into());
    }
    if patch.edits.is_empty() || patch.edits.len() > 128 {
        return Err("A patch requires 1..128 file edits".into());
    }
    let mut seen = BTreeSet::new();
    let mut prepared = Vec::new();
    for edit in patch.edits {
        let normalized = crate::project::normalize_path(&edit.path)?;
        if normalized != edit.path {
            return Err("Use canonical slash-separated paths".into());
        }
        if !seen.insert(edit.path.clone()) {
            return Err(format!("Duplicate file edit: {}", edit.path).into());
        }
        let path = project.resolve(&edit.path)?;
        let indexed = before.files.get(&edit.path);
        if !exact_hash(&edit.expected_hash, indexed.map(|f| f.hash.as_str())) {
            return Err(format!("Stale or missing expected_hash: {}", edit.path).into());
        }
        if indexed.is_none() {
            project.allows_new(&edit.path)?;
            if fs::symlink_metadata(&path).is_ok() {
                return Err(format!("Existing file is not indexed: {}", edit.path).into());
            }
        }
        let source = match (edit.source, edit.old, edit.new) {
            (Some(source), None, None) => source,
            (None, Some(old), Some(new)) => {
                let original = indexed.ok_or("New files require source, not match replacement")?;
                if old.is_empty() {
                    return Err("Match text must not be empty".into());
                }
                if original.text.find(&old).is_none()
                    || original.text.find(&old) != original.text.rfind(&old)
                {
                    return Err(format!("old must match exactly once: {}", edit.path).into());
                }
                original.text.replacen(&old, &new, 1)
            }
            _ => return Err("Each edit requires either source or both old and new".into()),
        };
        if source.len() > MAX_FILE as usize {
            return Err("Source exceeds the 2 MiB file limit".into());
        }
        if source.contains('\0') {
            return Err("Source must be UTF-8 text without NUL bytes".into());
        }
        let original = indexed.map(|f| f.text.as_bytes().to_vec());
        if let Some(bytes) = &original {
            if fs::read(&path)? != *bytes {
                return Err("Source changed during patch preparation".into());
            }
        }
        let mode = if original.is_some() {
            Some(fs::metadata(&path)?.permissions())
        } else {
            None
        };
        prepared.push(Prepared {
            relative: edit.path,
            path,
            source: source.into_bytes(),
            before: original,
            mode,
        });
    }
    let paths = prepared
        .iter()
        .map(|p| p.relative.clone())
        .collect::<Vec<_>>();
    tasks::check_claims(project, task, owner, &paths)?;
    if let (Some(task), Some(owner)) = (task, owner) {
        tasks::authorize(project, before, task, owner, &paths)?;
    }
    Ok(prepared)
}

fn destination_fresh(project: &Project, edit: &Prepared) -> Result<()> {
    project.resolve(&edit.relative)?;
    match &edit.before {
        Some(bytes) if fs::read(&edit.path)? != *bytes => {
            return Err(format!("Destination changed before commit: {}", edit.relative).into())
        }
        None => match fs::symlink_metadata(&edit.path) {
            Ok(_) => {
                return Err(
                    format!("New destination appeared before commit: {}", edit.relative).into(),
                )
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        },
        _ => {}
    }
    Ok(())
}
fn fresh(project: &Project, before: &Snapshot, prepared: &[Prepared]) -> Result<()> {
    if project.snapshot()?.hash != before.hash {
        return Err("Source/config changed before commit; refresh context".into());
    }
    for edit in prepared {
        destination_fresh(project, edit)?;
    }
    Ok(())
}
fn write_new(path: &Path, bytes: &[u8], mode: Option<&Permissions>) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(if mode.is_some() { 0o600 } else { 0o666 });
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    if let Some(mode) = mode {
        file.set_permissions(mode.clone())?;
    }
    file.sync_all()?;
    Ok(())
}
fn parents(project: &Project, path: &Path, created: &mut Vec<PathBuf>) -> Result<()> {
    let parent = path.parent().ok_or("Missing destination parent")?;
    let relative = parent.strip_prefix(&project.root)?;
    let mut cursor = project.root.clone();
    for component in relative.components() {
        cursor.push(component);
        match fs::symlink_metadata(&cursor) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => return Err("Destination parent is not a regular directory".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&cursor)?;
                created.push(cursor.clone());
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn cleanup(project: &Project, stage: &Path, created: &[PathBuf]) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Err(error) = fs::remove_dir_all(stage) {
        if error.kind() != std::io::ErrorKind::NotFound {
            warnings.push(format!(
                "Retained transaction directory {}: {error}",
                stage.display()
            ));
        }
    }
    for path in created.iter().rev() {
        let safe = (|| -> Result<()> {
            let relative = path.strip_prefix(&project.root)?;
            let mut current = project.root.clone();
            for component in relative.components() {
                current.push(component);
                let metadata = fs::symlink_metadata(&current)?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err("Directory ancestry changed".into());
                }
            }
            Ok(())
        })();
        if let Err(error) = safe {
            warnings.push(format!(
                "Unsafe directory cleanup {}: {error}",
                path.display()
            ));
            continue;
        }
        if let Err(error) = fs::remove_dir(path) {
            if error.kind() != std::io::ErrorKind::NotFound
                && error.kind() != std::io::ErrorKind::DirectoryNotEmpty
            {
                warnings.push(format!("Directory cleanup {}: {error}", path.display()));
            }
        }
    }
    warnings
}
fn rollback(
    project: &Project,
    stage: &Path,
    prepared: &[Prepared],
    committed: &[usize],
) -> Vec<String> {
    let mut errors = Vec::new();
    for &index in committed.iter().rev() {
        let edit = &prepared[index];
        // Revalidate parents as well as bytes: a renamed parent can be replaced
        // by a symlink after an earlier swap, even under a cooperating-agent lock.
        if let Err(error) = project.resolve(&edit.relative) {
            errors.push(format!(
                "Unsafe rollback destination {}: {error}",
                edit.relative
            ));
            continue;
        }
        // Do not overwrite a concurrent external editor during rollback.
        if fs::read(&edit.path).ok().as_deref() != Some(edit.source.as_slice()) {
            errors.push(format!(
                "Destination changed during rollback: {}",
                edit.path.display()
            ));
            continue;
        }
        let restored = if edit.before.is_some() {
            fs::rename(stage.join(format!("backup-{index}")), &edit.path)
        } else {
            fs::remove_file(&edit.path)
        };
        if let Err(error) = restored {
            errors.push(format!("Rollback {}: {error}", edit.path.display()));
        }
    }
    errors
}
fn transact<F>(
    project: &Project,
    before: &Snapshot,
    prepared: &[Prepared],
    mut before_swap: F,
) -> Result<Value>
where
    F: FnMut(usize) -> Result<()>,
{
    let identity = format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        SEQUENCE.fetch_add(1, Ordering::SeqCst)
    );
    let stage = project
        .state
        .join(format!("transaction-{}", &sha(identity)[..24]));
    fs::create_dir(&stage)?;
    let mut created = Vec::new();
    let mut committed = Vec::new();
    let result = (|| -> Result<Snapshot> {
        fresh(project, before, prepared)?;
        for (index, edit) in prepared.iter().enumerate() {
            write_new(
                &stage.join(format!("source-{index}")),
                &edit.source,
                edit.mode.as_ref(),
            )?;
            if let Some(bytes) = &edit.before {
                write_new(
                    &stage.join(format!("backup-{index}")),
                    bytes,
                    edit.mode.as_ref(),
                )?;
            }
        }
        fresh(project, before, prepared)?;
        for (index, edit) in prepared.iter().enumerate() {
            parents(project, &edit.path, &mut created)?;
            project.resolve(&edit.relative)?;
            before_swap(index)?;
            destination_fresh(project, edit)?;
            fs::rename(stage.join(format!("source-{index}")), &edit.path)?;
            committed.push(index);
        }
        let after = project.snapshot()?;
        for edit in prepared {
            if after.files.get(&edit.relative).map(|f| f.hash.as_str())
                != Some(sha(&edit.source).as_str())
            {
                return Err(format!("Committed source was not indexed: {}", edit.relative).into());
            }
        }
        Ok(after)
    })();
    match result {
        Ok(after) => {
            let warnings = cleanup(project, &stage, &[]);
            Ok(
                value!({"applied":true,"snapshot":after.hash,"files":prepared.iter().map(|p|value!({"path":p.relative,"hash":sha(&p.source),"created":p.before.is_none()})).collect::<Vec<_>>(),"cleanup_warnings":warnings}),
            )
        }
        Err(error) => {
            let failures = rollback(project, &stage, prepared, &committed);
            if !failures.is_empty() {
                return Err(format!(
                    "{error}; ROLLBACK_FAILED: {}; originals retained in {}",
                    failures.join("; "),
                    stage.display()
                )
                .into());
            }
            let warnings = cleanup(project, &stage, &created);
            Err(format!(
                "{error}; ordinary-I/O rollback completed{}",
                if warnings.is_empty() {
                    String::new()
                } else {
                    format!("; {}", warnings.join("; "))
                }
            )
            .into())
        }
    }
}

pub fn run(project: &Project, args: &[String]) -> Result<String> {
    let mut patch_path = None;
    let mut task = None;
    let mut owner = None;
    let mut index = 0;
    while index < args.len() {
        let flag = &args[index];
        index += 1;
        let argument = args
            .get(index)
            .ok_or("Expected a value after edit option")?
            .clone();
        index += 1;
        match flag.as_str() {
            "--patch" if patch_path.is_none() => patch_path = Some(argument),
            "--task" if task.is_none() => task = Some(argument),
            "--owner" if owner.is_none() => owner = Some(argument),
            _ => return Err("Usage: aienv apply --patch PATH|- [--task ID --owner NAME]".into()),
        }
    }
    if task.is_some() != owner.is_some() {
        return Err("--task and --owner must be supplied together".into());
    }
    let filename = patch_path.ok_or("Provide --patch PATH or --patch - for stdin")?;
    let mut bytes = Vec::new();
    if filename == "-" {
        std::io::stdin()
            .take(MAX_PATCH + 1)
            .read_to_end(&mut bytes)?;
    } else {
        File::open(filename)?
            .take(MAX_PATCH + 1)
            .read_to_end(&mut bytes)?;
    }
    if bytes.len() > MAX_PATCH as usize {
        return Err("Patch exceeds 16 MiB input limit".into());
    }
    let patch: Patch = serde_json::from_slice(&bytes)?;
    let before = project.snapshot()?;
    let prepared = prepare(project, &before, patch, task.as_deref(), owner.as_deref())?;
    json(&transact(project, &before, &prepared, |_| Ok(()))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        project: Project,
        path: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "aienv-edit-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir(&path).unwrap();
            let project = Project::init(&path).unwrap();
            fs::write(path.join("app.py"), "# Ω\r\nvalue = 1\r\n").unwrap();
            Self { project, path }
        }
        fn patch(&self, edits: Value) -> Patch {
            serde_json::from_value(
                value!({"snapshot":self.project.snapshot().unwrap().hash,"edits":edits}),
            )
            .unwrap()
        }
        fn existing(&self, old: &str, new: &str) -> Value {
            value!({"path":"app.py","expected_hash":sha(fs::read(self.path.join("app.py")).unwrap()),"old":old,"new":new})
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
    #[test]
    fn exact_replacement_and_new_nested_file_preserve_crlf() {
        let f = Fixture::new();
        let before = f.project.snapshot().unwrap();
        let patch=f.patch(value!([f.existing("value = 1","value = 2"),{"path":"lib/helper.txt","expected_hash":null,"source":"漢😀\n"}]));
        let edits = prepare(&f.project, &before, patch, None, None).unwrap();
        let result = transact(&f.project, &before, &edits, |_| Ok(())).unwrap();
        assert_eq!(result["applied"], true);
        assert_eq!(
            fs::read(f.path.join("app.py")).unwrap(),
            "# Ω\r\nvalue = 2\r\n".as_bytes()
        );
        assert_eq!(
            fs::read_to_string(f.path.join("lib/helper.txt")).unwrap(),
            "漢😀\n"
        );
    }
    #[test]
    fn stale_snapshot_filehash_and_ambiguous_match_reject_without_writes() {
        let f = Fixture::new();
        let before = f.project.snapshot().unwrap();
        let mut patch = f.patch(value!([f.existing("value = 1", "value = 2")]));
        patch.snapshot = "stale".into();
        assert!(prepare(&f.project, &before, patch, None, None).is_err());
        let mut bad = f.existing("value = 1", "value = 2");
        bad["expected_hash"] = value!("stale");
        assert!(prepare(&f.project, &before, f.patch(value!([bad])), None, None).is_err());
        fs::write(f.path.join("app.py"), "xx xx").unwrap();
        let now = f.project.snapshot().unwrap();
        assert!(prepare(
            &f.project,
            &now,
            f.patch(value!([f.existing("xx", "yy")])),
            None,
            None
        )
        .is_err());
        assert_eq!(fs::read_to_string(f.path.join("app.py")).unwrap(), "xx xx");
    }
    #[test]
    fn failed_os_rename_rolls_back_existing_and_new_files() {
        let f = Fixture::new();
        let before = f.project.snapshot().unwrap();
        let patch=f.patch(value!([f.existing("1","2"),{"path":"new/nested.txt","expected_hash":null,"source":"new"},{"path":"last.txt","expected_hash":null,"source":"last"}]));
        let edits = prepare(&f.project, &before, patch, None, None).unwrap();
        let error = transact(&f.project, &before, &edits, |i| {
            if i == 2 {
                // A real filesystem rename error, after two successful swaps.
                let stage = fs::read_dir(&f.project.state)?
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .find(|p| {
                        p.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with("transaction-")
                    })
                    .ok_or("Missing stage directory")?;
                fs::remove_file(stage.join("source-2"))?;
            }
            Ok(())
        })
        .unwrap_err();
        assert!(error.to_string().contains("rollback completed"));
        assert_eq!(
            fs::read(f.path.join("app.py")).unwrap(),
            before.files["app.py"].text.as_bytes()
        );
        assert!(!f.path.join("new").exists());
        assert!(!f.path.join("last.txt").exists());
        assert_eq!(f.project.snapshot().unwrap().hash, before.hash);
    }
    #[test]
    fn rollback_failure_retains_original_backups_and_reports_path() {
        let f = Fixture::new();
        let before = f.project.snapshot().unwrap();
        let patch = f.patch(value!([f.existing("1", "2"), {"path":"last.txt","expected_hash":null,"source":"last"}]));
        let edits = prepare(&f.project, &before, patch, None, None).unwrap();
        let error = transact(&f.project, &before, &edits, |index| {
            if index == 1 {
                fs::write(f.path.join("app.py"), "external edit")?;
                return Err(std::io::Error::other("injected swap failure").into());
            }
            Ok(())
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("ROLLBACK_FAILED"));
        let stage = fs::read_dir(&f.project.state)
            .unwrap()
            .map(|r| r.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("transaction-")
            })
            .unwrap();
        assert!(error.contains(stage.to_str().unwrap()));
        assert_eq!(
            fs::read(stage.join("backup-0")).unwrap(),
            before.files["app.py"].text.as_bytes()
        );
        assert_eq!(
            fs::read_to_string(f.path.join("app.py")).unwrap(),
            "external edit"
        );
    }

    #[test]
    fn overlapping_match_occurrences_and_missing_cas_reject() {
        let f = Fixture::new();
        fs::write(f.path.join("app.py"), "aaa").unwrap();
        let before = f.project.snapshot().unwrap();
        assert!(prepare(
            &f.project,
            &before,
            f.patch(value!([f.existing("aa", "b")])),
            None,
            None
        )
        .is_err());
        assert!(serde_json::from_value::<Patch>(
            value!({"snapshot":before.hash,"edits":[{"path":"new.txt","source":"new"}]})
        )
        .is_err());
    }

    #[test]
    fn ignored_new_files_and_existing_binary_files_are_not_patchable() {
        let f = Fixture::new();
        fs::write(f.path.join("binary.dat"), [0, 1, 2]).unwrap();
        let before = f.project.snapshot().unwrap();
        assert!(!before.files.contains_key("binary.dat"));
        assert!(prepare(
            &f.project,
            &before,
            f.patch(value!([{"path":"binary.dat","expected_hash":null,"source":"replacement"}])),
            None,
            None
        )
        .is_err());
        if std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok()
        {
            let status = std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(&f.path)
                .status()
                .unwrap();
            assert!(status.success());
            fs::write(f.path.join(".gitignore"), "ignored.txt\n").unwrap();
            let before = f.project.snapshot().unwrap();
            assert!(prepare(
                &f.project,
                &before,
                f.patch(value!([{"path":"ignored.txt","expected_hash":null,"source":"ignored"}])),
                None,
                None
            )
            .is_err());
            assert!(!f.path.join("ignored.txt").exists());
        }
    }

    #[test]
    fn active_claims_block_unscoped_and_wrong_owner_edits() {
        let f = Fixture::new();
        let args = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        tasks::run(
            &f.project,
            &args(&[
                "add",
                "change",
                "--title",
                "Change app",
                "--paths",
                "app.py",
            ]),
        )
        .unwrap();
        tasks::run(&f.project, &args(&["claim", "change", "--owner", "alice"])).unwrap();
        let before = f.project.snapshot().unwrap();
        assert!(prepare(
            &f.project,
            &before,
            f.patch(value!([f.existing("1", "2")])),
            None,
            None
        )
        .is_err());
        assert!(prepare(
            &f.project,
            &before,
            f.patch(value!([f.existing("1", "2")])),
            Some("change"),
            Some("bob")
        )
        .is_err());
        assert!(prepare(
            &f.project,
            &before,
            f.patch(value!([f.existing("1", "2")])),
            Some("change"),
            Some("alice")
        )
        .is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn failed_commit_directory_cleanup_does_not_follow_changed_parent_links() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let outside = Fixture::new();
        fs::create_dir(outside.path.join("nested")).unwrap();
        let before = f.project.snapshot().unwrap();
        let patch =
            f.patch(value!([{"path":"new/nested/file.txt","expected_hash":null,"source":"new"}]));
        let edits = prepare(&f.project, &before, patch, None, None).unwrap();
        let error = transact(&f.project, &before, &edits, |_| {
            fs::rename(f.path.join("new"), f.path.join("saved-parent"))?;
            symlink(&outside.path, f.path.join("new"))?;
            Err(std::io::Error::other("failure before first swap").into())
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("Unsafe directory cleanup"));
        assert!(outside.path.join("nested").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn rollback_rejects_symlink_parent_and_keeps_outside_file_unchanged() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let outside = Fixture::new();
        fs::create_dir(f.path.join("nested")).unwrap();
        fs::write(f.path.join("nested/file.txt"), "original").unwrap();
        fs::write(outside.path.join("file.txt"), "edited").unwrap();
        let before = f.project.snapshot().unwrap();
        let patch = f.patch(value!([
            {"path":"nested/file.txt","expected_hash":sha("original"),"source":"edited"},
            {"path":"last.txt","expected_hash":null,"source":"last"}
        ]));
        let edits = prepare(&f.project, &before, patch, None, None).unwrap();
        let error = transact(&f.project, &before, &edits, |index| {
            if index == 1 {
                fs::rename(f.path.join("nested"), f.path.join("saved-parent"))?;
                symlink(&outside.path, f.path.join("nested"))?;
                return Err(std::io::Error::other("replacement failure").into());
            }
            Ok(())
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("ROLLBACK_FAILED"));
        assert!(error.contains("Unsafe rollback destination"));
        assert_eq!(
            fs::read_to_string(outside.path.join("file.txt")).unwrap(),
            "edited"
        );
        let stage = fs::read_dir(&f.project.state)
            .unwrap()
            .map(|r| r.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("transaction-")
            })
            .unwrap();
        assert_eq!(
            fs::read_to_string(stage.join("backup-0")).unwrap(),
            "original"
        );
    }

    #[test]
    fn external_destination_edit_immediately_before_swap_is_preserved() {
        let f = Fixture::new();
        let before = f.project.snapshot().unwrap();
        let edits = prepare(
            &f.project,
            &before,
            f.patch(value!([f.existing("1", "2")])),
            None,
            None,
        )
        .unwrap();
        let error = transact(&f.project, &before, &edits, |_| {
            fs::write(f.path.join("app.py"), "external editor contents")?;
            Ok(())
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("Destination changed"));
        assert_eq!(
            fs::read_to_string(f.path.join("app.py")).unwrap(),
            "external editor contents"
        );
    }

    #[test]
    fn external_change_before_swap_is_detected() {
        let f = Fixture::new();
        let before = f.project.snapshot().unwrap();
        let edits = prepare(
            &f.project,
            &before,
            f.patch(value!([f.existing("1", "2")])),
            None,
            None,
        )
        .unwrap();
        fs::write(f.path.join("other.txt"), "new external source").unwrap();
        assert!(transact(&f.project, &before, &edits, |_| Ok(())).is_err());
        assert_eq!(
            fs::read(f.path.join("app.py")).unwrap(),
            before.files["app.py"].text.as_bytes()
        );
    }
    #[test]
    fn duplicate_mixed_and_outside_paths_reject() {
        let f = Fixture::new();
        let before = f.project.snapshot().unwrap();
        let edit = f.existing("1", "2");
        assert!(prepare(
            &f.project,
            &before,
            f.patch(value!([edit, edit])),
            None,
            None
        )
        .is_err());
        for path in [
            "../outside.txt",
            "/tmp/outside.txt",
            ".ai-env/config.json",
            ".git/config",
        ] {
            assert!(prepare(
                &f.project,
                &before,
                f.patch(value!([{"path":path,"expected_hash":null,"source":"bad"}])),
                None,
                None
            )
            .is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_hardlinks_and_modes() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let f = Fixture::new();
        fs::set_permissions(f.path.join("app.py"), Permissions::from_mode(0o751)).unwrap();
        let before = f.project.snapshot().unwrap();
        let edits = prepare(
            &f.project,
            &before,
            f.patch(value!([f.existing("1", "2")])),
            None,
            None,
        )
        .unwrap();
        transact(&f.project, &before, &edits, |_| Ok(())).unwrap();
        assert_eq!(
            fs::metadata(f.path.join("app.py"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o751
        );
        symlink(f.path.join("app.py"), f.path.join("link.py")).unwrap();
        assert!(prepare(
            &f.project,
            &f.project.snapshot().unwrap(),
            f.patch(value!([{"path":"link.py","expected_hash":null,"source":"bad"}])),
            None,
            None
        )
        .is_err());
        fs::hard_link(f.path.join("app.py"), f.path.join("hard.py")).unwrap();
        assert!(f.project.resolve("app.py").is_err());
    }
}
