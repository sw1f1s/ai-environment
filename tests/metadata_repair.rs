use aienv::metadata_repair;
use serde_json::{json, Value};

fn reads() -> Value {
    json!({"refs":[{"id":"r2","path":"tests"},{"id":"r1","path":"src/policy.py"}]})
}
fn plan() -> Value {
    json!({"snapshot":"original-snapshot","mode":"team","checks":["test"],
        "acceptance":["Keep authoritative policy"],"goal":"FULL_GOAL_SENTINEL",
        "summary":"Saved implementation decision","architecture":{"boundaries":["Keep local ownership"]},
        "edits":[{"path":"src/policy.py","source":"CODE_PAYLOAD_SENTINEL"}],
        "tasks":[
            {"id":"change","title":"Change the existing caller","instructions":"Read the current policy before editing",
                "paths":["src"],"depends":[],"context":["Read policy.py before coding"]},
            {"id":"tests","title":"Check the caller","instructions":"Reuse existing discovery",
                "paths":["regression_tests"],"depends":["change"],"read_refs":["r2"]}]})
}
fn sources() -> Value {
    json!({"refs":[
        {"id":"s1","path":"src/policy.py","line":4,"end_line":9,"obligations":["policy"]},
        {"id":"s2","path":"tests/test_call.py","line":12,"end_line":19,"obligations":["callers"]}],
        "by_obligation":{"policy":["s1"],"callers":["s2"]}})
}
fn qa() -> Value {
    json!({"snapshot":"original-snapshot","verdict":"accept","proofs":["p1"],
        "tasks":[],"findings":[],"assessment":["SAVED_ASSESSMENT"],
        "architecture":{"assessment":["Existing boundary retained"],"issues":[]},
        "verification":[
            {"id":"policy","status":"verified","proofs":["p1"],"evidence":"SAVED_EVIDENCE: source and discovered regression support the boundary",
                "locations":[{"path":"not-indexed.py","line":99999}]},
            {"id":"callers","status":"verified","proofs":["p1"],"evidence":"SAVED_CALLER_EVIDENCE","source_refs":["s2"]}]})
}
fn specification(role: &str, original: &Value, catalog: &Value) -> Value {
    metadata_repair::prepare(role, original, catalog)
        .unwrap()
        .expect("selector-only correction expected")
}
fn patch(id: &str, refs: Value) -> Value {
    json!({"snapshot":"original-snapshot","selections":[{"id":id,"refs":refs,"unresolved":false}]})
}

#[test]
fn plan_correction_preserves_every_saved_decision_and_code_payload() {
    let original = plan();
    let saved = original.clone();
    let spec = specification("coordinator", &original, &reads());
    assert_eq!(spec["kind"], "plan_reads");
    assert_eq!(spec["rows"].as_array().unwrap().len(), 1);
    assert_eq!(spec["rows"][0]["id"], "change");
    let merged = metadata_repair::merge(&original, &patch("change", json!(["r1"])), &spec).unwrap();
    let mut expected = original.clone();
    expected["tasks"][0]
        .as_object_mut()
        .unwrap()
        .remove("context");
    expected["tasks"][0]["read_refs"] = json!(["r1"]);
    assert_eq!(merged, expected);
    assert_eq!(original, saved);
    assert_eq!(merged["tasks"][1], original["tasks"][1]);
    let prompt = metadata_repair::prompt(&spec, "Unknown read selector").unwrap();
    assert!(prompt.starts_with("AI Env role: coordinator."));
    for forbidden in [
        "FULL_GOAL_SENTINEL",
        "CODE_PAYLOAD_SENTINEL",
        "architecture",
        "profile",
        "expected_hash",
    ] {
        assert!(
            !prompt.contains(forbidden),
            "Leaked saved implementation context: {forbidden}"
        );
    }
    assert!(prompt.contains("No tools") && prompt.contains("unresolved=true"));
}

#[test]
fn qa_correction_changes_only_reference_fields_and_keeps_semantic_verdict() {
    let original = qa();
    let saved = original.clone();
    let spec = specification("qa", &original, &sources());
    assert_eq!(spec["kind"], "qa_sources");
    assert_eq!(
        spec["rows"][0]["evidence"],
        original["verification"][0]["evidence"]
    );
    let merged = metadata_repair::merge(&original, &patch("policy", json!(["s1"])), &spec).unwrap();
    let mut expected = original.clone();
    expected["verification"][0]
        .as_object_mut()
        .unwrap()
        .remove("locations");
    expected["verification"][0]["source_refs"] = json!(["s1"]);
    assert_eq!(merged, expected);
    assert_eq!(original, saved);
    assert_eq!(merged["verification"][1], original["verification"][1]);
    let prompt = metadata_repair::prompt(&spec, "unindexed location").unwrap();
    assert!(prompt.starts_with("AI Env role: qa."));
    assert!(prompt.contains("SAVED_EVIDENCE") && !prompt.contains("SAVED_ASSESSMENT"));
}

#[test]
fn valid_reference_fields_do_not_cause_an_assessment_retry() {
    let mut p = plan();
    p["tasks"][0].as_object_mut().unwrap().remove("context");
    p["tasks"][0]["read_refs"] = json!(["r1"]);
    assert!(metadata_repair::prepare("architect", &p, &reads())
        .unwrap()
        .is_none());
    let mut q = qa();
    q["verification"][0]
        .as_object_mut()
        .unwrap()
        .remove("locations");
    q["verification"][0]["source_refs"] = json!(["s1"]);
    assert!(metadata_repair::prepare("qa", &q, &sources())
        .unwrap()
        .is_none());
    assert!(metadata_repair::prepare("developer", &plan(), &reads())
        .unwrap()
        .is_none());
}

#[test]
fn missing_unknown_and_duplicate_ids_are_selector_errors() {
    for refs in [None, Some(json!(["unknown"])), Some(json!(["r1", "r1"]))] {
        let mut original = plan();
        let task = original["tasks"][0].as_object_mut().unwrap();
        task.remove("context");
        if let Some(refs) = refs {
            task.insert("read_refs".into(), refs);
        }
        let spec = specification("coordinator", &original, &reads());
        let merged = metadata_repair::merge(&original, &patch("change", json!([])), &spec).unwrap();
        assert_eq!(merged["tasks"][0]["read_refs"], json!([]));
    }
    let mut original = qa();
    original["verification"][0]
        .as_object_mut()
        .unwrap()
        .remove("locations");
    original["verification"][0]["source_refs"] = json!(["s2"]);
    let spec = specification("qa", &original, &sources());
    assert!(metadata_repair::merge(&original, &patch("policy", json!(["s2"])), &spec).is_err());
    assert!(metadata_repair::merge(&original, &patch("policy", json!(["s1"])), &spec).is_ok());
}

#[test]
fn unknown_original_owners_and_semantic_or_proof_shapes_are_not_repaired() {
    for mode in 0..8 {
        let mut original = qa();
        match mode {
            0 => original["verification"][0]["id"] = json!("unknown"),
            1 => original["verification"][1]["id"] = json!("policy"),
            2 => {
                original["verification"].as_array_mut().unwrap().pop();
            }
            3 => original["verification"][0]["status"] = json!("passed"),
            4 => original["verification"][0]["proofs"] = json!(["not-a-supplied-proof"]),
            5 => original["verification"][0]["evidence"] = json!(""),
            6 => original["verification"][0]["proofs"] = json!([]),
            _ => original["verdict"] = json!("probably-accept"),
        }
        assert!(
            metadata_repair::prepare("qa", &original, &sources())
                .unwrap()
                .is_none(),
            "mode {mode}"
        );
    }
    for mode in 0..4 {
        let mut original = plan();
        match mode {
            0 => original["tasks"][1]["id"] = json!("change"),
            1 => original["tasks"][0]["depends"] = json!(["unknown-task"]),
            2 => original["tasks"][0]["depends"] = json!(["tests"]),
            _ => original["tasks"][0]["paths"] = json!(["../outside"]),
        }
        assert!(
            metadata_repair::prepare("coordinator", &original, &reads())
                .unwrap()
                .is_none(),
            "mode {mode}"
        );
    }
}

#[test]
fn qa_with_no_choices_needs_semantic_gap_instead_of_fake_verified_refs() {
    let mut catalog = sources();
    catalog["refs"].as_array_mut().unwrap().remove(0);
    catalog["by_obligation"]["policy"] = json!([]);
    assert!(metadata_repair::prepare("qa", &qa(), &catalog)
        .unwrap()
        .is_none());
    let mut original = qa();
    original["verdict"] = json!("changes");
    original["verification"][0]["status"] = json!("gap");
    original["verification"][0]["proofs"] = json!([]);
    let spec = specification("qa", &original, &catalog);
    let merged = metadata_repair::merge(&original, &patch("policy", json!([])), &spec).unwrap();
    assert_eq!(merged["verdict"], "changes");
    assert_eq!(merged["verification"][0]["status"], "gap");
    let schema = metadata_repair::schema(&spec).unwrap();
    assert_eq!(
        schema["properties"]["selections"]["items"]["anyOf"][0]["properties"]["refs"]["maxItems"],
        0
    );
}

#[test]
fn unresolved_is_never_treated_as_success_and_original_is_unchanged() {
    let original = qa();
    let spec = specification("qa", &original, &sources());
    let mut patch = patch("policy", json!([]));
    patch["selections"][0]["unresolved"] = json!(true);
    let error = metadata_repair::merge(&original, &patch, &spec)
        .unwrap_err()
        .to_string();
    assert!(error.starts_with("Metadata repair cannot resolve saved references"));
    assert_eq!(original, qa());
}

#[test]
fn patch_rejects_wrong_owner_snapshot_refs_duplicates_counts_and_extra_fields() {
    let original = qa();
    let spec = specification("qa", &original, &sources());
    for mode in 0..10 {
        let mut p = patch("policy", json!(["s1"]));
        match mode {
            0 => p["snapshot"] = json!("latest-not-original"),
            1 => p["selections"][0]["id"] = json!("callers"),
            2 => p["selections"][0]["refs"] = json!(["s2"]),
            3 => p["selections"][0]["refs"] = json!(["unknown"]),
            4 => p["selections"][0]["refs"] = json!(["s1", "s1"]),
            5 => p["selections"] = json!([]),
            6 => p["selections"] = json!([p["selections"][0], p["selections"][0]]),
            7 => p["selections"][0]["evidence"] = json!("invented assurance"),
            8 => p["verdict"] = json!("changes"),
            _ => p["selections"][0]["unresolved"] = json!("false"),
        }
        assert!(
            metadata_repair::merge(&original, &p, &spec).is_err(),
            "mode {mode}"
        );
    }
}

#[test]
fn complete_selector_schema_is_strict_and_owner_specific() {
    let spec = specification("qa", &qa(), &sources());
    let schema = metadata_repair::schema(&spec).unwrap();
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(
        schema["properties"]["snapshot"]["enum"],
        json!(["original-snapshot"])
    );
    assert_eq!(schema["properties"]["selections"]["minItems"], 1);
    assert_eq!(schema["properties"]["selections"]["maxItems"], 1);
    let row = &schema["properties"]["selections"]["items"]["anyOf"][0];
    assert_eq!(row["additionalProperties"], false);
    assert_eq!(row["properties"]["id"]["enum"], json!(["policy"]));
    assert_eq!(row["properties"]["refs"]["items"]["enum"], json!(["s1"]));
    assert_eq!(row["properties"]["refs"]["maxItems"], 8);
}

#[test]
fn corrupt_catalog_or_spec_is_rejected_before_a_repair_call() {
    for mode in 0..6 {
        let mut cards = sources();
        match mode {
            0 => cards["refs"][1]["id"] = json!("s1"),
            1 => cards["by_obligation"]["policy"] = json!(["s2"]),
            2 => cards["refs"][0]["obligations"] = json!(["unknown"]),
            3 => cards["refs"][0]["path"] = json!("../outside"),
            4 => cards["refs"][0]["line"] = json!(0),
            _ => cards["refs"][0]["end_line"] = json!(1),
        }
        assert!(
            metadata_repair::prepare("qa", &qa(), &cards).is_err(),
            "catalog mode {mode}"
        );
    }
    let original = qa();
    let good = specification("qa", &original, &sources());
    for mode in 0..7 {
        let mut spec = good.clone();
        match mode {
            0 => spec["rows"][0]["id"] = json!("unknown"),
            1 => spec["choices"]["policy"] = json!(["s2"]),
            2 => spec["choices"]["unrequested"] = json!([]),
            3 => spec["rows"].as_array_mut().unwrap().clear(),
            4 => spec["rows"][0]["evidence"] = json!("Changed saved meaning"),
            5 => spec["snapshot"] = json!("different-snapshot"),
            _ => spec["unexpected"] = json!("new-semantic-field"),
        }
        assert!(
            metadata_repair::merge(&original, &patch("policy", json!(["s1"])), &spec).is_err(),
            "spec mode {mode}"
        );
    }
}

#[test]
fn bounds_are_explicit_and_utf8_excerpts_do_not_copy_unbounded_context() {
    let mut p = plan();
    p["tasks"][0]["instructions"] = json!("雪😀".repeat(500));
    let spec = specification("coordinator", &p, &reads());
    assert!(spec["rows"][0]["instructions"].as_str().unwrap().len() <= 512);
    assert!(
        spec["rows"][0]["omitted_instruction_bytes"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(
        metadata_repair::prompt(&spec, &"ошибка😀".repeat(2000))
            .unwrap()
            .len()
            < 37000
    );
    let mut excessive = plan();
    excessive["tasks"][0]["read_refs"] = json!(vec!["unknown"; 65]);
    assert!(metadata_repair::prepare("coordinator", &excessive, &reads()).is_err());
    let mut q = qa();
    q["verification"][0]["source_refs"] = json!(vec!["s1"; 9]);
    assert!(metadata_repair::prepare("qa", &q, &sources()).is_err());
    let mut huge = reads();
    huge["refs"] = json!((0..128)
        .map(|i| json!({"id":format!("r{i}"),"path":format!("{i}/{}","a".repeat(390))}))
        .collect::<Vec<_>>());
    let error = metadata_repair::prepare("coordinator", &plan(), &huge)
        .unwrap_err()
        .to_string();
    assert!(error.contains("32 KiB"));
}

#[test]
fn snapshot_mismatch_and_changed_saved_meaning_are_not_silently_rebased() {
    let original = plan();
    let spec = specification("coordinator", &original, &reads());
    let mut changed = original.clone();
    changed["tasks"][0]["instructions"] = json!("Different requested read ownership");
    assert!(metadata_repair::merge(&changed, &patch("change", json!(["r1"])), &spec).is_err());
    changed = original.clone();
    changed["snapshot"] = json!("new-snapshot");
    assert!(metadata_repair::merge(&changed, &patch("change", json!(["r1"])), &spec).is_err());
    let mut cards = reads();
    cards["snapshot"] = json!("new-snapshot");
    assert!(metadata_repair::prepare("coordinator", &original, &cards)
        .unwrap()
        .is_none());
}

#[test]
fn every_affected_owner_is_selected_once_even_with_the_correct_total_count() {
    let mut original = qa();
    original["verification"][1]["source_refs"] = json!(["unknown"]);
    let spec = specification("qa", &original, &sources());
    let mut proposal = patch("policy", json!(["s1"]));
    proposal["selections"] = json!([
        {"id":"policy","refs":["s1"],"unresolved":false},
        {"id":"policy","refs":["s1"],"unresolved":false}
    ]);
    assert!(metadata_repair::merge(&original, &proposal, &spec).is_err());
    proposal["selections"][1] = json!({"id":"callers","refs":["s2"],"unresolved":false});
    let merged = metadata_repair::merge(&original, &proposal, &spec).unwrap();
    assert_eq!(merged["verification"][1]["source_refs"], json!(["s2"]));
    assert_eq!(merged["assessment"], original["assessment"]);
}

#[test]
fn corrupted_display_metadata_cannot_reach_the_selector_prompt() {
    let good = specification("qa", &qa(), &sources());
    for mode in 0..4 {
        let mut spec = good.clone();
        match mode {
            0 => spec["rows"][0]["selector_field"] = json!("read_refs"),
            1 => spec["rows"][0]["selected"] = json!([{"source":"unbounded payload"}]),
            2 => spec["rows"][0]["selected"] = json!([{"path":"a","line":1,"extra":"field"}]),
            _ => {
                spec["rows"][0]["selector_field"] = json!("missing");
                spec["rows"][0]["selected"] = json!(["unexpected"]);
            }
        }
        assert!(metadata_repair::prompt(&spec, "invalid source IDs").is_err());
        assert!(metadata_repair::schema(&spec).is_err());
    }
}
