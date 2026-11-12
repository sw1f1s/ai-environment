use crate::Result;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::{
    fd::AsRawFd,
    unix::fs::{MetadataExt, OpenOptionsExt},
};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_FILE: u64 = 2 * 1024 * 1024;
const MAX_FILES: usize = 20_000;
const MAX_TOTAL: usize = 128 * 1024 * 1024;
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub checks: BTreeMap<String, Check>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub argv: Vec<String>,
    #[serde(default = "timeout")]
    pub timeout_seconds: u64,
}
fn timeout() -> u64 {
    120
}
#[derive(Clone, Debug, Serialize)]
pub struct FileEntry {
    pub path: String,
    pub hash: String,
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub hash: String,
    pub files: BTreeMap<String, FileEntry>,
    pub omitted: usize,
}
impl Snapshot {
    pub fn scope_hash(&self, paths: &[String]) -> String {
        let mut bytes = Vec::new();
        let mut scopes = paths.to_vec();
        scopes.sort();
        scopes.dedup();
        for scope in &scopes {
            bytes.extend_from_slice(scope.as_bytes());
            bytes.push(0);
        }
        for (path, file) in &self.files {
            if scopes.iter().any(|scope| scope_contains(scope, path)) {
                bytes.extend_from_slice(path.as_bytes());
                bytes.push(0);
                bytes.extend_from_slice(file.hash.as_bytes());
                bytes.push(0);
            }
        }
        sha(bytes)
    }
}
#[derive(Clone, Debug)]
pub struct Project {
    pub root: PathBuf,
    pub state: PathBuf,
}
pub fn sha(data: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(data.as_ref()))
}
pub fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
pub fn scope_contains(scope: &str, path: &str) -> bool {
    scope == "."
        || path == scope
        || path
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with('/'))
}
fn excluded_component(value: &str) -> bool {
    matches!(
        value,
        ".git"
            | ".ai-env"
            | ".agents"
            | ".codex"
            | ".tools"
            | "target"
            | "node_modules"
            | "obj"
            | ".vs"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".build"
            | "dist"
            | "build"
            | ".next"
            | ".cache"
            | ".ssh"
            | ".aws"
    )
}
fn excluded_file(value: &str) -> bool {
    value == ".env"
        || value.starts_with(".env.")
        || value.ends_with(".pem")
        || value.ends_with(".key")
        || matches!(value, "credentials.json" | ".DS_Store")
}
pub fn normalize_path(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 2048
        || value.chars().any(char::is_control)
        || value.contains('\\')
    {
        return Err("Expected a normal relative project path".into());
    }
    let path = Path::new(value);
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or("Path must be UTF-8")?;
                if excluded_component(part) || excluded_file(part) {
                    return Err(format!("Path is outside indexed scope: {value}").into());
                }
                parts.push(part);
            }
            _ => return Err("Absolute paths and traversal are not allowed".into()),
        }
    }
    if parts.is_empty() {
        return Err("Empty path".into());
    }
    Ok(parts.join("/"))
}
fn plain_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!("Expected regular file: {}", path.display()).into());
    }
    #[cfg(unix)]
    if metadata.nlink() != 1 {
        return Err(format!("Hardlinked files are unsupported: {}", path.display()).into());
    }
    Ok(())
}
fn directory(path: &Path) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(format!("Expected unlinked directory: {}", path.display()).into());
        }
    } else {
        fs::create_dir(path)?;
    }
    Ok(())
}
fn nofollow(options: &mut OpenOptions) {
    #[cfg(target_os = "macos")]
    options.custom_flags(0x100).mode(0o600);
    #[cfg(target_os = "linux")]
    options.custom_flags(0x20000).mode(0o600);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = options;
    }
}
pub struct ProjectLock {
    _file: File,
}
impl Project {
    pub fn init(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        if !root.is_dir() {
            return Err("Project root must be a directory".into());
        }
        let project = Self {
            state: root.join(".ai-env"),
            root,
        };
        directory(&project.state)?;
        let _lock = project.lock()?;
        let config = project.state.join("config.json");
        if config.exists() || fs::symlink_metadata(&config).is_ok() {
            project.config()?;
        } else {
            project.write_state("config.json", &Config::default())?;
        }
        if !project.state.join(".gitignore").exists() {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            nofollow(&mut options);
            options
                .open(project.state.join(".gitignore"))?
                .write_all(b"*\n")?;
        }
        Ok(project)
    }
    pub fn open(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let project = Self {
            state: root.join(".ai-env"),
            root,
        };
        let metadata =
            fs::symlink_metadata(&project.state).map_err(|_| "Run aienv init --root PATH first")?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("Invalid project state directory".into());
        }
        project.config()?;
        Ok(project)
    }
    pub fn lock(&self) -> Result<ProjectLock> {
        directory(&self.state)?;
        let path = self.state.join("lock");
        if fs::symlink_metadata(&path).is_ok() {
            plain_file(&path)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        nofollow(&mut options);
        let file = options.open(&path)?;
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn flock(fd: i32, operation: i32) -> i32;
            }
            if unsafe { flock(file.as_raw_fd(), 2) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let actual = fs::symlink_metadata(&path)?;
            let opened = file.metadata()?;
            if actual.ino() != opened.ino() || actual.dev() != opened.dev() || opened.nlink() != 1 {
                return Err("Project lock was replaced".into());
            }
        }
        #[cfg(not(unix))]
        return Err("This release requires macOS or Linux process locks".into());
        Ok(ProjectLock { _file: file })
    }
    fn state_path(&self, name: &str, create: bool) -> Result<PathBuf> {
        if name.is_empty() || name.contains('\\') {
            return Err("Invalid state key".into());
        }
        directory(&self.state)?;
        let parts: Vec<_> = Path::new(name).components().collect();
        let mut path = self.state.clone();
        for (index, part) in parts.iter().enumerate() {
            let Component::Normal(part) = part else {
                return Err("Invalid state key".into());
            };
            path.push(part);
            if index + 1 < parts.len() {
                if create {
                    directory(&path)?;
                } else if let Ok(meta) = fs::symlink_metadata(&path) {
                    if meta.file_type().is_symlink() || !meta.is_dir() {
                        return Err("Invalid state ancestor".into());
                    }
                }
            }
        }
        if fs::symlink_metadata(&path).is_ok() {
            plain_file(&path)?;
        }
        Ok(path)
    }
    pub fn read_state<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T> {
        let path = self.state_path(name, false)?;
        match fs::read(&path) {
            Ok(bytes) => {
                if bytes.len() > 8 * 1024 * 1024 {
                    return Err("State too large".into());
                }
                Ok(serde_json::from_slice(&bytes)?)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn write_state<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        let encoded = serde_json::to_vec_pretty(value)?;
        if encoded.len() >= 8 * 1024 * 1024 {
            return Err("State exceeds the 8 MiB read limit".into());
        }
        let path = self.state_path(name, true)?;
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let temp = path.with_file_name(format!(".tmp-{}-{nonce}", std::process::id()));
        let result = (|| -> Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            nofollow(&mut options);
            let mut file = options.open(&temp)?;
            file.write_all(&encoded)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            if fs::symlink_metadata(&path).is_ok() {
                plain_file(&path)?;
            }
            fs::rename(&temp, &path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
    pub fn config(&self) -> Result<Config> {
        let path = self.state_path("config.json", false)?;
        plain_file(&path)?;
        let bytes = fs::read(path)?;
        if bytes.len() > 1024 * 1024 {
            return Err("Config too large".into());
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
    pub fn resolve(&self, value: &str) -> Result<PathBuf> {
        let path = normalize_path(value)?;
        let mut current = self.root.clone();
        for part in path.split('/') {
            current.push(part);
            if let Ok(meta) = fs::symlink_metadata(&current) {
                if meta.file_type().is_symlink() {
                    return Err("Linked source paths are unsupported".into());
                }
                if current != self.root.join(&path) && !meta.is_dir() {
                    return Err("Source ancestor is not a directory".into());
                }
                if current == self.root.join(&path) {
                    plain_file(&current)?;
                }
            }
        }
        Ok(current)
    }
    pub fn allows_new(&self, path: &str) -> Result<()> {
        normalize_path(path)?;
        if self.git_root() {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["check-ignore", "--no-index", "--", path])
                .output()?;
            if output.status.success() {
                return Err("New file is ignored by Git and outside indexed scope".into());
            }
            if output.status.code() != Some(1) {
                return Err("Unable to check Git ignore rules".into());
            }
        }
        Ok(())
    }
    fn git_root(&self) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| Path::new(s.trim()).canonicalize().ok())
            .is_some_and(|p| self.root.starts_with(p))
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        let mut paths = Vec::new();
        let mut omitted = 0;
        if self.git_root() {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args([
                    "ls-files",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                    "-z",
                    "--",
                    ".",
                ])
                .output()?;
            if !output.status.success() {
                return Err("Git file discovery failed".into());
            }
            for path in output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
                let path = std::str::from_utf8(path)?.to_string();
                if normalize_path(&path).is_ok() {
                    paths.push(path);
                } else {
                    omitted += 1;
                }
            }
        } else {
            self.walk(&self.root, &mut paths, &mut omitted)?;
        }
        paths.sort();
        paths.dedup();
        if paths.len() > MAX_FILES {
            return Err("Project exceeds 20000 candidate files; use a smaller project root".into());
        }
        let mut files = BTreeMap::new();
        let mut total = 0;
        for path in paths {
            let absolute = self.root.join(&path);
            let metadata = match fs::symlink_metadata(&absolute) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_FILE
            {
                omitted += 1;
                continue;
            }
            if self.resolve(&path).is_err() {
                omitted += 1;
                continue;
            }
            let bytes = fs::read(absolute)?;
            total += bytes.len();
            if total > MAX_TOTAL {
                return Err("Indexed project exceeds 128 MiB; use a smaller project root".into());
            }
            if bytes.contains(&0) {
                omitted += 1;
                continue;
            }
            let hash = sha(&bytes);
            let text = match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(_) => {
                    omitted += 1;
                    continue;
                }
            };
            files.insert(path.clone(), FileEntry { path, hash, text });
        }
        let mut digest = Sha256::new();
        digest.update(b"ai-env-snapshot-1\0");
        digest.update(fs::read(self.state_path("config.json", false)?)?);
        for (path, entry) in &files {
            digest.update(path.as_bytes());
            digest.update([0]);
            digest.update(entry.hash.as_bytes());
            digest.update([0]);
        }
        Ok(Snapshot {
            hash: format!("{:x}", digest.finalize()),
            files,
            omitted,
        })
    }
    fn walk(&self, dir: &Path, paths: &mut Vec<String>, omitted: &mut usize) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path
                .strip_prefix(&self.root)?
                .to_str()
                .ok_or("Project path is not UTF-8")?
                .to_string();
            if normalize_path(&relative).is_err() {
                *omitted += 1;
                continue;
            }
            let ty = entry.file_type()?;
            if ty.is_symlink() {
                *omitted += 1;
                continue;
            }
            if ty.is_dir() {
                self.walk(&path, paths, omitted)?;
            } else if ty.is_file() {
                paths.push(relative);
            }
            if paths.len() > MAX_FILES {
                return Err("Project exceeds 20000 files; use a smaller root".into());
            }
        }
        Ok(())
    }
}
