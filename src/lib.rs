pub mod brief;
pub mod consistency;
pub mod context;
pub mod edit;
pub mod graph;
pub mod metadata_repair;
pub mod model_edits;
pub mod model_plans;
pub mod notes;
pub mod orchestrate;
pub mod profile;
pub mod project;
pub mod qa_context;
pub mod retrieval;
mod source_paths;
pub mod source_refs;
pub mod tasks;
pub mod verification;
pub mod verify;
pub mod workflow;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub fn json<T: serde::Serialize>(value: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)?)
}
