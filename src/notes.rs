use crate::{
    json,
    project::{normalize_path, valid_id, Project},
    Result,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Default, Serialize, Deserialize)]
pub struct Notes {
    entries: BTreeMap<String, Note>,
}
#[derive(Serialize, Deserialize)]
struct Note {
    text: String,
    paths: Vec<String>,
    scope_hash: String,
}
pub fn run(project: &Project, args: &[String]) -> Result<String> {
    if args.is_empty() || args[0] == "list" {
        return list(project);
    }
    if args[0] == "show" {
        if args.len() != 2 || !valid_id(&args[1]) {
            return Err("Usage: note show ID".into());
        }
        let value: serde_json::Value = serde_json::from_str(&list(project)?)?;
        let note = value["notes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == args[1])
            .ok_or("Unknown note ID")?;
        return json(&serde_json::json!({"snapshot":value["snapshot"],"note":note}));
    }
    if args[0] != "set" || args.len() < 4 {
        return Err("Usage: note set ID --text TEXT [--paths path,dir] | note list".into());
    }
    let id = &args[1];
    if !valid_id(id) {
        return Err("Invalid note ID".into());
    }
    let mut text = None;
    let mut paths = vec![".".to_string()];
    let mut index = 2;
    while index < args.len() {
        let value = args.get(index + 1).ok_or("Missing note option value")?;
        match args[index].as_str() {
            "--text" => text = Some(value.clone()),
            "--paths" => {
                paths = value
                    .split(',')
                    .map(|p| {
                        if p == "." {
                            Ok(p.to_string())
                        } else {
                            normalize_path(p)
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;
            }
            _ => return Err("Unknown note option".into()),
        }
        index += 2;
    }
    let text = text.ok_or("Missing --text")?;
    paths.sort();
    paths.dedup();
    if text.trim().is_empty() || text.len() > 4000 || paths.is_empty() || paths.len() > 64 {
        return Err("Note must contain 1..4000 bytes and a scope".into());
    }
    let snapshot = project.snapshot()?;
    let mut notes: Notes = project.read_state("notes.json")?;
    if notes.entries.len() >= 256 && !notes.entries.contains_key(id) {
        return Err("Note limit reached".into());
    }
    notes.entries.insert(
        id.clone(),
        Note {
            text,
            scope_hash: snapshot.scope_hash(&paths),
            paths,
        },
    );
    project.write_state("notes.json", &notes)?;
    json(&serde_json::json!({"saved":true,"id":id,"snapshot":snapshot.hash}))
}
pub fn list(project: &Project) -> Result<String> {
    let snapshot = project.snapshot()?;
    let notes: Notes = project.read_state("notes.json")?;
    let entries:Vec<_>=notes.entries.iter().map(|(id,n)|serde_json::json!({"id":id,"text":n.text,"paths":n.paths,"stale":snapshot.scope_hash(&n.paths)!=n.scope_hash})).collect();
    json(&serde_json::json!({"snapshot":snapshot.hash,"notes":entries}))
}
