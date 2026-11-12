//! Compiler-backed graph and bounded, selected semantic context.
mod cache;
pub mod model;
mod process;
mod query;
use crate::{
    json,
    project::{normalize_path, Project},
    Result,
};
pub use cache::load_current;
pub(crate) use cache::{freshness, load_at, load_for_brief, RefreshBudget};
use model::{language, Options, Spec};
pub use model::{Envelope, Graph};
use std::path::Path;
const HELP:&str="aienv graph build --language c|cpp|csharp|rust|python|typescript|javascript [--project PATH] [--file REL]* [--flag ARG]* [--compile-commands PATH] [--reference DLL]* [--define SYMBOL]* [--libclang PATH]\naienv graph refresh|status --language LANGUAGE\naienv graph find QUERY --language LANGUAGE [--budget BYTES]\naienv graph impact SYMBOL [--language LANGUAGE] [--depth 1..3] [--budget BYTES]\naienv context --symbol SYMBOL [--relations calls,callers,types,used-by,references,inheritance] [--body] [--depth 1..3] [--budget BYTES]\n";
fn selected_language(args: &[String]) -> Result<String> {
    let mut selected = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--language" {
            if selected.is_some() {
                return Err("Repeated --language".into());
            }
            i += 1;
            selected = Some(args.get(i).ok_or("Missing language")?.clone());
        } else {
            return Err("Use only --language LANGUAGE for this command".into());
        }
        i += 1;
    }
    let selected =
        selected.ok_or("Specify --language c|cpp|csharp|rust|python|typescript|javascript")?;
    language(&selected)?;
    Ok(selected)
}
fn absolute(project: &Project, value: &str) -> Result<String> {
    let path = Path::new(value);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project.root.join(path)
    };
    if !path.is_file() {
        return Err(format!("Semantic input does not exist: {}", path.display()).into());
    }
    Ok(path.to_string_lossy().into_owned())
}
fn spec(project: &Project, args: &[String]) -> Result<Spec> {
    let mut options = Options::default();
    let mut lang = None;
    let mut seen = std::collections::BTreeSet::new();
    let mut i = 0;
    while i < args.len() {
        let flag = &args[i];
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("Missing value for {flag}"))?;
        match flag.as_str() {
            "--language" => {
                if lang.is_some() {
                    return Err("Repeated --language".into());
                }
                language(value)?;
                lang = Some(value.clone());
            }
            "--file" => {
                let path = normalize_path(value)?;
                if path != *value {
                    return Err("Use normalized relative --file paths".into());
                }
                options.files.push(path);
            }
            "--flag" => options.flags.push(value.clone()),
            "--define" => options.defines.push(value.clone()),
            "--reference" => options.references.push(absolute(project, value)?),
            "--compile-commands" | "--libclang" | "--project" => {
                if !seen.insert(flag.clone()) {
                    return Err(format!("Repeated {flag}").into());
                }
                if flag == "--project" {
                    options.project = Some(absolute(project, value)?);
                } else if flag == "--libclang" {
                    options.libclang = Some(absolute(project, value)?);
                } else {
                    options.compile_commands = Some(absolute(project, value)?);
                }
            }
            _ => return Err(format!("Unknown graph build option {flag}").into()),
        }
        i += 2;
    }
    let lang = lang.ok_or("Specify --language c|cpp|csharp|rust|python|typescript|javascript")?;
    if !matches!(lang.as_str(), "c" | "cpp" | "csharp")
        && (!options.flags.is_empty()
            || !options.references.is_empty()
            || !options.defines.is_empty()
            || options.compile_commands.is_some()
            || options.libclang.is_some())
    {
        return Err("Rust/Python/TypeScript/JavaScript use --project/--file; compiler-specific C/C++/C# options do not apply".into());
    }
    if matches!(lang.as_str(), "c" | "cpp" | "csharp") && options.project.is_some() {
        return Err(
            "--project applies to Rust/Python/TypeScript/JavaScript; see semantic setup".into(),
        );
    }
    if lang == "csharp"
        && (!options.flags.is_empty()
            || options.compile_commands.is_some()
            || options.libclang.is_some())
    {
        return Err("C# uses --define/--reference, not Clang flags/database".into());
    }
    if lang != "csharp" && (!options.references.is_empty() || !options.defines.is_empty()) {
        return Err("C/C++ defines use --flag -DNAME; --reference/--define are C# options".into());
    }
    if options.files.len() > 1024
        || options.flags.len() > 512
        || options.references.len() > 1024
        || options.defines.len() > 512
    {
        return Err("Too many semantic options".into());
    }
    Ok(Spec {
        language: lang,
        options,
    })
}
fn summary(cached: &Envelope, fresh: bool, reason: Option<String>) -> serde_json::Value {
    serde_json::json!({"language":cached.spec.language,"index":cached.index_hash,"fresh":fresh,"stale_reason":reason,"analysis_complete":cached.graph.analysis_complete,"analyzer":cached.graph.analyzer,"nodes":cached.graph.nodes.len(),"edges":cached.graph.edges.len(),"unresolved":cached.graph.unresolved.len(),"diagnostic_count":cached.graph.diagnostics.len(),"diagnostics":cached.graph.diagnostics.iter().take(20).collect::<Vec<_>>(),"limitations":cached.graph.limitations,"source_key":cached.source_key,"options":cached.spec.options})
}
pub fn run(project: &Project, args: &[String]) -> Result<String> {
    let Some(command) = args.first() else {
        return Ok(HELP.into());
    };
    match command.as_str() {
        "--help" | "help" => Ok(HELP.into()),
        "build" => {
            let spec = spec(project, &args[1..])?;
            let cached = cache::build(project, spec)?;
            json(&summary(&cached, true, None))
        }
        "refresh" => {
            let lang = selected_language(&args[1..])?;
            let cached = cache::read(project, &lang)?;
            let cached = cache::build(project, cached.spec)?;
            json(&summary(&cached, true, None))
        }
        "status" => {
            let lang = selected_language(&args[1..])?;
            let cached = cache::read(project, &lang)?;
            let snapshot = project.snapshot()?;
            let error = cache::freshness(project, &snapshot, &cached)
                .err()
                .map(|e| e.to_string());
            json(&summary(&cached, error.is_none(), error))
        }
        "find" => query::find(project, &args[1..]),
        "impact" => query::impact(project, &args[1..]),
        _ => Err(format!("Unknown graph command {command}; graph --help").into()),
    }
}
pub fn context(project: &Project, args: &[String]) -> Result<String> {
    query::context(project, args)
}
pub fn is_cursor(value: &str) -> bool {
    value.starts_with("g1-")
}
