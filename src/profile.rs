//! Static, source-bound style and test evidence. No project code is executed.
use crate::{
    project::{normalize_path, scope_contains, FileEntry, Project},
    Result,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

fn parent(path: &str) -> &str {
    path.rsplit_once('/')
        .map_or(".", |(directory, _)| directory)
}
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
fn selected(paths: &[String], path: &str) -> bool {
    paths.is_empty() || paths.iter().any(|scope| scope_contains(scope, path))
}
fn source(path: &str) -> bool {
    matches!(
        Path::new(path).extension().and_then(|s| s.to_str()),
        Some(
            "rs" | "py"
                | "c"
                | "cc"
                | "cpp"
                | "cxx"
                | "h"
                | "hpp"
                | "cs"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "mjs"
                | "go"
                | "java"
                | "kt"
                | "swift"
        )
    )
}
fn style_config(path: &str) -> bool {
    let name = basename(path);
    matches!(
        name,
        ".editorconfig"
            | "rustfmt.toml"
            | ".rustfmt.toml"
            | ".clang-format"
            | ".clang-tidy"
            | ".prettierrc"
            | ".prettierrc.json"
            | ".prettierrc.yaml"
            | ".prettierrc.yml"
            | ".prettierrc.toml"
            | "prettier.config.js"
            | "prettier.config.cjs"
            | "prettier.config.mjs"
            | ".eslintrc"
            | ".eslintrc.json"
            | ".eslintrc.yml"
            | ".eslintrc.yaml"
            | ".eslintrc.js"
            | ".eslintrc.cjs"
            | "eslint.config.js"
            | "eslint.config.mjs"
            | "eslint.config.cjs"
            | "eslint.config.ts"
            | "biome.json"
            | "biome.jsonc"
            | "ruff.toml"
            | ".ruff.toml"
            | "pyproject.toml"
            | "package.json"
            | "setup.cfg"
            | "tox.ini"
            | ".flake8"
    )
}
fn scope_relevant(scope: &str, paths: &[String]) -> bool {
    paths.is_empty()
        || paths
            .iter()
            .any(|path| scope_contains(scope, path) || scope_contains(path, scope))
}
fn prefix(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}
fn config_content(file: &FileEntry) -> Option<(String, &'static str)> {
    if basename(&file.path) == "package.json" {
        let value: Value = serde_json::from_str(&file.text).ok()?;
        let keys = ["prettier", "eslintConfig", "biome"];
        let values: serde_json::Map<String, Value> = keys
            .into_iter()
            .filter_map(|key| value.get(key).map(|v| (key.into(), v.clone())))
            .collect();
        return (!values.is_empty())
            .then(|| (Value::Object(values).to_string(), "selected_json_keys"));
    }
    if basename(&file.path) == "pyproject.toml" {
        let mut relevant = false;
        let mut lines = Vec::new();
        for line in file.text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                relevant = [
                    "[tool.ruff",
                    "[tool.black",
                    "[tool.isort",
                    "[tool.autopep8",
                    "[tool.yapf",
                    "[tool.pylint",
                ]
                .iter()
                .any(|p| trimmed.starts_with(p));
            }
            if relevant {
                lines.push(line);
            }
        }
        return (!lines.is_empty()).then(|| (lines.join("\n"), "selected_toml_sections"));
    }
    Some((file.text.clone(), "raw_config_not_executed"))
}
fn evidence(file: &FileEntry) -> Value {
    json!({"path":file.path,"hash":file.hash})
}
fn kind(path: &str) -> &'static str {
    if path.split('/').any(|part| {
        matches!(
            part.to_ascii_lowercase().as_str(),
            "integration" | "integrationtests" | "integration_tests" | "e2e"
        )
    }) || path.contains(".integration.")
    {
        "integration"
    } else if path.split('/').any(|part| {
        matches!(
            part.to_ascii_lowercase().as_str(),
            "unit" | "unittests" | "unit_tests"
        )
    }) || path.contains(".unit.")
    {
        "unit"
    } else {
        "unknown"
    }
}
fn test_file(file: &FileEntry) -> Option<(&'static str, &'static str)> {
    let path = &file.path;
    let name = basename(path);
    let ext = Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if name == "CMakeLists.txt"
        && file.text.lines().any(|line| {
            line.trim_start()
                .to_ascii_lowercase()
                .starts_with("add_test(")
        })
    {
        return Some((
            "unknown",
            "CMake add_test declaration; conditions/build directory not evaluated",
        ));
    }
    if ext == "rs" && path.split('/').any(|part| part == "tests") {
        return Some((
            "integration",
            "Rust tests directory; execution not verified",
        ));
    }
    if ext == "rs"
        && (file.text.contains("#[test]")
            || file.text.contains("#[tokio::test]")
            || file.text.contains("#[async_std::test]"))
    {
        return Some(("unit", "inline Rust test attribute"));
    }
    if ext == "csproj"
        && (file.text.contains("Microsoft.NET.Test.Sdk")
            || file.text.contains("<IsTestProject>true</IsTestProject>")
            || file.text.contains("xunit")
            || file.text.contains("NUnit")
            || file.text.contains("MSTest.TestFramework"))
    {
        return Some((kind(path), "test-project marker in csproj"));
    }
    if ext == "cs"
        && ["[Fact]", "[Theory]", "[Test]", "[TestMethod]", "[TestCase("]
            .iter()
            .any(|marker| file.text.contains(marker))
    {
        return Some((kind(path), "C# test-framework attribute"));
    }
    if ext == "py" && (name.starts_with("test_") || name.ends_with("_test.py")) {
        return Some((kind(path), "Python test filename convention"));
    }
    if ext == "go" && name.ends_with("_test.go") {
        return Some((kind(path), "Go test filename convention"));
    }
    if matches!(ext, "java" | "kt")
        && (file.text.contains("@Test")
            || file.text.contains("org.junit")
            || file.text.contains("kotlin.test")
            || name.ends_with("Test.java")
            || name.ends_with("Tests.kt"))
    {
        return Some((kind(path), "Java/Kotlin test filename or framework marker"));
    }
    if ext == "swift"
        && (file.text.contains("import XCTest")
            || file.text.contains("import Testing")
            || name.ends_with("Tests.swift"))
    {
        return Some((kind(path), "Swift test filename or framework import"));
    }
    if matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs")
        && (name.contains(".test.") || name.contains(".spec."))
    {
        return Some((kind(path), "JavaScript/TypeScript test filename convention"));
    }
    if matches!(ext, "c" | "cc" | "cpp" | "cxx" | "h" | "hpp")
        && (name.starts_with("test_")
            || name.contains("_test.")
            || file.text.contains("TEST_CASE(")
            || file.text.contains("TEST(")
            || file.text.contains("TEST_F("))
    {
        return Some((kind(path), "C/C++ test filename or framework macro"));
    }
    if source(path)
        && path.split('/').any(|part| {
            matches!(
                part.to_ascii_lowercase().as_str(),
                "tests" | "test" | "integration_tests"
            )
        })
    {
        return Some((
            kind(path),
            "Test-directory candidate; runner/framework unknown",
        ));
    }
    None
}
fn observations(files: &[&FileEntry]) -> Vec<Value> {
    let mut groups: BTreeMap<&str, Vec<&FileEntry>> = BTreeMap::new();
    for file in files {
        groups.entry(parent(&file.path)).or_default().push(file);
    }
    groups.into_iter().map(|(scope, files)| {
        let mut indentation = BTreeSet::new();
        let mut newline = BTreeSet::new();
        let sampled: Vec<_> = files.iter().take(4).copied().collect();
        let mut sample_truncated = false;
        for file in &sampled {
            let text = prefix(&file.text,8192);
            sample_truncated |= text.len() < file.text.len();
            if text.contains("\r\n") { newline.insert("crlf"); }
            if text.as_bytes().iter().enumerate().any(|(i,b)|*b==b'\n' && (i==0 || text.as_bytes()[i-1]!=b'\r')) { newline.insert("lf"); }
            for line in text.lines() {
                if line.trim().is_empty() { continue; }
                let indent = line.len()-line.trim_start_matches([' ', '\t']).len();
                if indent == 0 { continue; }
                if line[..indent].contains('\t') { indentation.insert("tabs".to_owned()); }
                if line[..indent].contains(' ') { indentation.insert(format!("spaces:{indent}")); }
            }
        }
        let indent = if indentation.len()==1 { indentation.iter().next().unwrap().clone() } else if indentation.is_empty() { "unknown".into() } else { "mixed_widths_or_nesting".into() };
        let newline = if newline.len()==1 { *newline.iter().next().unwrap() } else if newline.is_empty() { "unknown" } else { "mixed" };
        json!({"scope":scope,"inferred":true,"files":sampled.iter().map(|file|&file.path).collect::<Vec<_>>(),"evidence":sampled.iter().map(|file|evidence(file)).collect::<Vec<_>>(),"indentation":indent,"newline":newline,"naming":"unknown; inspect analogous declarations","sample_truncated":sample_truncated,"files_not_sampled":files.len()-sampled.len()})
    }).collect()
}

/// Build deterministic, byte-bounded evidence from the same source snapshot as other AI Env tools.
pub fn run(project: &Project, args: &[String]) -> Result<String> {
    let mut budget = 12_000usize;
    let mut seen_budget = false;
    let mut paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--budget" if !seen_budget => {
                i += 1;
                budget = args.get(i).ok_or("Missing --budget value")?.parse()?;
                seen_budget = true;
            }
            "--path" => {
                i += 1;
                let path = args.get(i).ok_or("Missing --path value")?;
                paths.push(if path == "." {
                    path.clone()
                } else {
                    normalize_path(path)?
                });
            }
            _ => return Err("Expected profile [--path FILE_OR_PREFIX] [--budget BYTES]".into()),
        }
        i += 1;
    }
    if !(2048..=24_000).contains(&budget)
        || paths.len() > 64
        || paths.iter().map(String::len).sum::<usize>() > 2048
    {
        return Err("Profile budget must be 2048..24000; scopes at most 64 and 2048 bytes".into());
    }
    paths.sort();
    paths.dedup();
    let snapshot = project.snapshot()?;
    let config = project.config()?;
    let mut configs = Vec::new();
    let mut content_omitted = 0usize;
    let mut tests = Vec::new();
    let mut candidates = Vec::new();
    let files: Vec<_> = snapshot
        .files
        .values()
        .filter(|file| source(&file.path) && selected(&paths, &file.path))
        .collect();
    for file in snapshot.files.values() {
        if style_config(&file.path) && scope_relevant(parent(&file.path), &paths) {
            if let Some((text, selection)) = config_content(file) {
                let included = prefix(&text, 2048);
                content_omitted += text.len() - included.len();
                configs.push(json!({"path":file.path,"scope":parent(&file.path),"depth":file.path.matches('/').count(),"content":included,"content_omitted_bytes":text.len()-included.len(),"selection":selection,"evidence":evidence(file)}));
            }
        }
        if let Some((kind, reason)) = test_file(file) {
            tests.push(json!({"path":file.path,"kind":kind,"evidence":reason,"hash":file.hash,"in_requested_scope":selected(&paths,&file.path)}));
        }
        if basename(&file.path) == "Cargo.toml" {
            let library = if parent(&file.path) == "." {
                "src/lib.rs".to_owned()
            } else {
                format!("{}/src/lib.rs", parent(&file.path))
            };
            if snapshot.files.contains_key(&library)
                || file.text.lines().any(|line| line.trim() == "[lib]")
            {
                candidates.push(json!({"argv":["cargo","test","--offline","--locked","--manifest-path",file.path,"--lib"],"kind":"unit","evidence":evidence(file),"availability":"candidate; toolchain and cached dependencies not verified"}));
            }
            candidates.push(json!({"argv":["cargo","test","--offline","--locked","--manifest-path",file.path,"--tests"],"kind":"unknown","evidence":evidence(file),"availability":"candidate; target mix and cached dependencies not verified"}));
        }
        if basename(&file.path) == "package.json" {
            if let Ok(value) = serde_json::from_str::<Value>(&file.text) {
                if value
                    .get("scripts")
                    .and_then(|s| s.get("test"))
                    .is_some_and(Value::is_string)
                {
                    candidates.push(json!({"argv":["npm","--prefix",parent(&file.path),"run","test"],"kind":"unknown","evidence":evidence(file),"availability":"candidate; project script not executed or audited"}));
                }
            }
        }
        if file.path.ends_with(".csproj") && test_file(file).is_some() {
            candidates.push(json!({"argv":["dotnet","test",file.path,"--no-restore"],"kind":kind(&file.path),"evidence":evidence(file),"availability":"candidate; dependencies and SDK not verified"}));
        }
        if file.path.ends_with(".py")
            && test_file(file).is_some()
            && (file.text.contains("import unittest") || file.text.contains("from unittest import"))
        {
            candidates.push(json!({"argv":["python3","-m","unittest","discover","-s",parent(&file.path)],"kind":kind(&file.path),"evidence":evidence(file),"availability":"candidate; discovery/package imports not executed"}));
        }
    }
    // Parent configurations are presented before nested ones. Globs/format-specific
    // merge and inheritance rules remain the responsibility of the native formatter.
    configs.sort_by_key(|value| {
        (
            value["depth"].as_u64().unwrap_or(0),
            value["path"].as_str().unwrap_or("").to_owned(),
        )
    });
    let mut configured: Vec<_> = config.checks.iter().map(|(name,check)|json!({"name":name,"argv":check.argv,"timeout_seconds":check.timeout_seconds})).collect();
    let mut candidate_commands = BTreeSet::new();
    candidates.retain(|candidate| candidate_commands.insert(candidate["argv"].to_string()));
    let totals = [
        configs.len(),
        files.len(),
        tests.len(),
        candidates.len(),
        configured.len(),
    ];
    let mut observed = observations(&files);
    let observed_total = observed.len();
    configs.truncate(16);
    observed.truncate(8);
    tests.truncate(32);
    candidates.truncate(16);
    configured.truncate(16);
    let omitted = json!({"style_configs":totals[0]-configs.len(),"observations":observed_total-observed.len(),"discovered":totals[2]-tests.len(),"candidates":totals[3]-candidates.len(),"configured":totals[4]-configured.len()});
    let mut out = json!({"snapshot":snapshot.hash,"style":{"configs":configs,"observations":observed,"precedence":"Explicit repository rules override inferred observations. Configs ordered parent to child; native tool globs/inheritance are not resolved."},"testing":{"configured":configured,"discovered":tests,"candidates":candidates,"unconfigured":config.checks.is_empty(),"status":"Static evidence only; discovery is not execution or proof of complete coverage."},"coverage":{"indexed_files":snapshot.files.len(),"snapshot_excluded":snapshot.omitted,"requested_paths":paths,"source_files_in_scope":totals[1],"style_configs_found":totals[0],"tests_found":totals[2],"candidates_found":totals[3],"configured_found":totals[4],"config_content_omitted_bytes":content_omitted,"omitted":omitted,"complete":false}});
    let trim = [
        ("style", "observations", "observations"),
        ("testing", "candidates", "candidates"),
        ("testing", "discovered", "discovered"),
        ("style", "configs", "style_configs"),
        ("testing", "configured", "configured"),
    ];
    loop {
        let encoded = serde_json::to_string(&out)?;
        if encoded.len() <= budget {
            if project.snapshot()?.hash != snapshot.hash {
                return Err("Project changed during profile discovery; retry".into());
            }
            return Ok(encoded);
        }
        let mut removed = false;
        for (section, key, count) in trim {
            if let Some(array) = out[section][key].as_array_mut() {
                if array.pop().is_some() {
                    let old = out["coverage"]["omitted"][count].as_u64().unwrap_or(0);
                    out["coverage"]["omitted"][count] = json!(old + 1);
                    removed = true;
                    break;
                }
            }
        }
        if !removed {
            return Err("Profile metadata exceeds budget; narrow --path scopes".into());
        }
    }
}
