use aienv::{
    brief, context, edit, graph, json, notes, orchestrate, profile, project::Project, retrieval,
    tasks, verify, workflow, Result,
};
use std::{env, path::PathBuf};
const HELP:&str="AI Env — project tools for coding agents\n\nUsage: aienv [--root PATH] COMMAND\n\n  init                         Initialize local .ai-env state (no source changes)\n  brief [QUERY]                Read one adaptive source packet\n  query [--query TEXT]          Project stdin task into bounded navigation terms\n  profile [--path PREFIX]       Read source-bound project style and test evidence\n  orchestrate ID --request FILE  Run native adaptive model orchestration\n  workflow ACTION [ID]          Architect/developers/reviewer/QA state and gates\n  context [QUERY] [OPTIONS]     Read compact text matches or selected source\n  index [QUERY]                Read file/hash overview\n  graph build|refresh|status|find|impact ...  Static symbol/type/call graph\n  apply --patch FILE           Apply snapshot-checked source edits (or - for stdin)\n  verify CHECK                 Run a named configured check, persist actual proof\n  task add|claim|complete|release|list|show ...\n  note set|list ...            Store source-bound decisions\n  status | handoff              Resume tasks, decisions and current verification\n\nContext: --path FILE --start N --end N --budget BYTES --cursor TOKEN\nApply:   [--task ID --owner NAME]\nChecks are fixed argv in .ai-env/config.json; no model, API or editor required.\n";
fn execute(mut args: Vec<String>) -> Result<String> {
    if args.is_empty() || matches!(args[0].as_str(), "help" | "--help" | "-h") {
        return Ok(HELP.to_string());
    }
    if args[0] == "--version" {
        return Ok(format!("aienv {}", env!("CARGO_PKG_VERSION")));
    }
    let mut root = env::current_dir()?;
    let mut index = 0;
    let mut seen = false;
    while index < args.len() {
        // This stateless command owns its trailing arguments, including a
        // precise query whose literal value may itself be "--root".
        if index == 0 && args[index] == "query" {
            break;
        }
        if args[index] == "--" {
            break;
        }
        if args[index] == "--root" {
            if seen {
                return Err("--root may only be provided once".into());
            }
            seen = true;
            root = PathBuf::from(args.get(index + 1).ok_or("Missing --root PATH")?);
            args.drain(index..=index + 1);
        } else {
            index += 1;
        }
    }
    let command = args.first().ok_or("Missing command")?.as_str();
    let tail = &args[1..];
    if command == "__orchestrate-model" {
        return orchestrate::worker(tail);
    }
    if command == "query" {
        return retrieval::run(tail);
    }
    if command == "init" {
        if !tail.is_empty() {
            return Err("init accepts only --root PATH".into());
        }
        let project = Project::init(&root)?;
        return json(
            &serde_json::json!({"initialized":true,"root":project.root,"config":project.state.join("config.json"),"checks":project.config()?.checks.keys().collect::<Vec<_>>()}),
        );
    }
    let project = Project::open(&root)?;
    if command == "orchestrate" {
        return orchestrate::run(&project, tail);
    }
    let _lock = project.lock()?;
    match command {
        "brief" => brief::run(&project, tail),
        "profile" => profile::run(&project, tail),
        "workflow" => workflow::run(&project, tail),
        "context"
            if tail.iter().any(|a| a == "--symbol")
                || tail
                    .windows(2)
                    .any(|w| w[0] == "--cursor" && graph::is_cursor(&w[1])) =>
        {
            graph::context(&project, tail)
        }
        "context" => context::run(&project, tail),
        "graph" => graph::run(&project, tail),
        "index" => context::index(&project, tail),
        "apply" => edit::run(&project, tail),
        "verify" => verify::run(&project, tail),
        "task" => tasks::run(&project, tail),
        "note" => notes::run(&project, tail),
        "status" | "handoff" => {
            if !tail.is_empty() {
                return Err("status/handoff accepts no options".into());
            }
            let task_state: serde_json::Value =
                serde_json::from_str(&tasks::status(&project, &[])?)?;
            let notes: serde_json::Value = serde_json::from_str(&notes::list(&project)?)?;
            compact_handoff(&project, task_state, notes)
        }
        _ => Err(format!("Unknown command {command}; use aienv --help").into()),
    }
}

fn compact_handoff(
    project: &Project,
    mut tasks: serde_json::Value,
    mut notes: serde_json::Value,
) -> Result<String> {
    use serde_json::{json as value, Value};
    fn clip(value: &Value, length: usize) -> String {
        value.as_str().unwrap_or("").chars().take(length).collect()
    }
    let mut workflows: Value = serde_json::from_str(&workflow::run(project, &["list".into()])?)?;
    let workflow_total = workflows["workflows"].as_array().map_or(0, Vec::len)
        + workflows["omitted"].as_u64().unwrap_or(0) as usize;
    if let Some(entries) = workflows["workflows"].as_array_mut() {
        entries.truncate(6);
        for entry in entries {
            let progress = entry["task_states"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            let mut counts = std::collections::BTreeMap::new();
            for task in progress.values() {
                *counts
                    .entry(task["status"].as_str().unwrap_or("unknown").to_owned())
                    .or_insert(0usize) += 1;
            }
            entry.as_object_mut().unwrap().remove("task_states");
            entry["task_counts"] = value!(counts);
            entry["details"] = value!(format!(
                "workflow status {}",
                entry["id"].as_str().unwrap_or("")
            ));
        }
    }
    let mut all = tasks["tasks"].as_array().cloned().unwrap_or_default();
    let mut counts = std::collections::BTreeMap::new();
    for task in &all {
        *counts
            .entry(task["status"].as_str().unwrap_or("unknown").to_string())
            .or_insert(0usize) += 1;
    }
    all.sort_by_key(|t| match t["status"].as_str() {
        Some("running") => 0,
        Some("stale") => 1,
        Some("pending") => 2,
        _ => 3,
    });
    tasks["task_count"] = value!(all.len());
    tasks["counts"] = value!(counts);
    tasks["tasks"] = value!(all
        .iter()
        .take(12)
        .map(|t| {
            let mut row = t.clone();
            row["title"] = value!(clip(&t["title"], 160));
            row["title_truncated"] =
                value!(t["title"].as_str().unwrap_or("").chars().count() > 160);
            for field in ["paths", "depends"] {
                let items = t[field].as_array().cloned().unwrap_or_default();
                row[format!("{field}_count")] = value!(items.len());
                row[field] = value!(items
                    .iter()
                    .take(4)
                    .map(|v| clip(v, 120))
                    .collect::<Vec<_>>());
            }
            row["summary"] = value!(true);
            row
        })
        .collect::<Vec<_>>());
    tasks["fresh_proof_count"] = value!(tasks["fresh_proofs"].as_array().map_or(0, Vec::len));
    if let Some(proofs) = tasks["fresh_proofs"].as_array_mut() {
        proofs.truncate(4);
    }
    let entries = notes["notes"].as_array().cloned().unwrap_or_default();
    notes["note_count"] = value!(entries.len());
    notes["notes"] = value!(entries
        .iter()
        .take(8)
        .map(
            |n| value!({"id":n["id"],"stale":n["stale"],"text":clip(&n["text"],200),"summary":true})
        )
        .collect::<Vec<_>>());
    loop {
        tasks["omitted_tasks"] = value!(all.len() - tasks["tasks"].as_array().unwrap().len());
        notes["omitted_notes"] = value!(entries.len() - notes["notes"].as_array().unwrap().len());
        workflows["omitted"] =
            value!(workflow_total - workflows["workflows"].as_array().map_or(0, Vec::len));
        let output = json(
            &value!({"project":project.root,"tasks":tasks,"decisions":notes,"workflows":workflows,"details":"task show ID; task list; note show ID; note list; workflow list; workflow status ID"}),
        )?;
        if output.len() < 16000 {
            // Reserve the CLI's terminal newline.
            return Ok(output);
        }
        if !notes["notes"].as_array().unwrap().is_empty() {
            notes["notes"].as_array_mut().unwrap().pop();
        } else if !tasks["tasks"].as_array().unwrap().is_empty() {
            tasks["tasks"].as_array_mut().unwrap().pop();
        } else if !workflows["workflows"].as_array().unwrap().is_empty() {
            workflows["workflows"].as_array_mut().unwrap().pop();
        } else {
            return Err("Project metadata exceeds handoff output budget".into());
        }
    }
}

fn main() {
    match execute(env::args().skip(1).collect()) {
        Ok(output) => {
            let failed = serde_json::from_str::<serde_json::Value>(&output)
                .ok()
                .is_some_and(|v| v.get("passed") == Some(&serde_json::Value::Bool(false)));
            print!("{output}");
            if !output.ends_with('\n') {
                println!();
            }
            if failed {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(2);
        }
    }
}
