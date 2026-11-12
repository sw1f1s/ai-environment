//! End-to-end compiler facts on real C/C++/C# fixtures, never a mocked analyzer.
use aienv::project::{sha, Project};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
const REPO: &str = env!("CARGO_MANIFEST_DIR");

fn backend(language: &str) -> bool {
    let script = Path::new(REPO)
        .join("skills/ai-env/scripts/analyzers")
        .join(if language == "csharp" {
            "csharp.py"
        } else {
            "clang.py"
        });
    let ready = script.is_file()
        && (language != "csharp"
            || Path::new(REPO)
                .join("skills/ai-env/scripts/bin/roslyn/GraphAnalyzer.dll")
                .is_file());
    if !ready {
        assert_ne!(
            std::env::var("AI_ENV_REQUIRE_SEMANTIC_BACKENDS").as_deref(),
            Ok("1"),
            "Required real {language} backend is missing; build the adapters before semantic QA"
        );
        eprintln!("SKIP real {language} semantic QA: adapter artifacts are not built");
    }
    ready
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    language: String,
}
impl Fixture {
    fn new(language: &str, partial: bool) -> Self {
        let base = std::env::temp_dir().join(format!(
            "ai-env semantic fixture {}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let root = base.join("project with spaces");
        let source = Path::new(REPO)
            .join("tests/fixtures/semantic")
            .join(if partial {
                format!("partial/{language}")
            } else {
                language.into()
            });
        copy_tree(&source, &root);
        let fixture = Self {
            base,
            root,
            language: language.into(),
        };
        fixture.okay(&["init"]);
        fixture
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_aienv"))
            .args(["--root", self.root.to_str().unwrap()])
            .args(args)
            .output()
            .unwrap()
    }
    fn okay(&self, args: &[&str]) -> String {
        let result = self.run(args);
        assert!(
            result.status.success(),
            "{args:?}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap()
    }
    fn build(&self) -> Value {
        let mut args = vec!["graph", "build", "--language", self.language.as_str()];
        if self.language != "csharp" {
            args.extend([
                "--flag",
                "-Iinclude",
                "--flag",
                if self.language == "c" {
                    "-std=c11"
                } else {
                    "-std=c++20"
                },
            ]);
        }
        self.okay(&args);
        self.cache()
    }
    fn cache(&self) -> Value {
        serde_json::from_slice(
            &fs::read(
                self.root
                    .join(format!(".ai-env/semantic/{}.json", self.language)),
            )
            .unwrap(),
        )
        .unwrap()
    }
    fn context(&self, id: &str, extra: &[&str]) -> String {
        let mut args = vec!["context", "--symbol", id];
        args.extend_from_slice(extra);
        self.okay(&args)
    }
    fn adapter(&self, overrides: &[(&str, String)]) -> Value {
        let snapshot = Project::open(&self.root).unwrap().snapshot().unwrap();
        let files: Vec<_> = snapshot
            .files
            .values()
            .map(|file| {
                let text = overrides
                    .iter()
                    .find(|(path, _)| *path == file.path)
                    .map(|(_, text)| text.as_str())
                    .unwrap_or(&file.text);
                json!({"path":file.path,"hash":sha(text),"text":text})
            })
            .collect();
        let script = Path::new(REPO)
            .join("skills/ai-env/scripts/analyzers")
            .join(if self.language == "csharp" {
                "csharp.py"
            } else {
                "clang.py"
            });
        let mut child = Command::new("python3")
            .arg(script)
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let request = json!({"schema_version":1,"language":self.language,"root":self.root,"files":files,"options":{"files":[],"flags":if self.language=="csharp"{vec![]}else{vec!["-Iinclude",if self.language=="c"{"-std=c11"}else{"-std=c++20"}]},"compile_commands":null,"libclang":null,"references":[],"defines":[]}});
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&request).unwrap())
            .unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(
            result.status.success(),
            "adapter stderr={}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&result.stdout).unwrap_or_else(|e| {
            panic!(
                "Adapter did not emit one JSON: {e}; stdout={}",
                String::from_utf8_lossy(&result.stdout)
            )
        })
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn nodes(graph: &Value) -> &[Value] {
    graph["nodes"].as_array().unwrap()
}
fn edges(graph: &Value) -> &[Value] {
    graph["edges"].as_array().unwrap()
}
fn id(node: &Value) -> &str {
    node["id"].as_str().unwrap()
}
fn qualified(node: &Value) -> &str {
    node["qualified_name"]
        .as_str()
        .unwrap()
        .trim_start_matches("global::")
}
fn matches_qualified(node: &Value, name: &str) -> bool {
    qualified(node) == name
        || qualified(node)
            .strip_prefix(name)
            .is_some_and(|suffix| suffix.starts_with('(') || suffix.starts_with('<'))
}
fn named<'a>(graph: &'a Value, name: &str) -> Vec<&'a Value> {
    nodes(graph)
        .iter()
        .filter(|n| matches_qualified(n, name))
        .collect()
}
fn one<'a>(graph: &'a Value, name: &str) -> &'a Value {
    let found = named(graph, name);
    assert_eq!(found.len(), 1, "Expected one {name}; candidates={found:?}");
    found[0]
}
fn edge<'a>(graph: &'a Value, source: &Value, target: &Value, kind: &str) -> &'a Value {
    let found: Vec<_> = edges(graph)
        .iter()
        .filter(|e| e["source"] == id(source) && e["target"] == id(target) && e["kind"] == kind)
        .collect();
    assert!(
        !found.is_empty(),
        "Missing {kind}: {} -> {}",
        qualified(source),
        qualified(target)
    );
    found[0]
}
fn schema_and_provenance(graph: &Value, language: &str) {
    assert_eq!(graph["schema_version"], 1);
    assert_eq!(graph["language"], language);
    assert!(!graph["analyzer"]["version"].as_str().unwrap().is_empty());
    assert_eq!(
        graph["analyzer"]["name"],
        if language == "csharp" {
            "roslyn"
        } else {
            "libclang"
        }
    );
    eprintln!(
        "semantic-real: {language}, {} {}, nodes={}, edges={}",
        graph["analyzer"]["name"],
        graph["analyzer"]["version"],
        nodes(graph).len(),
        edges(graph).len()
    );
    let ids: BTreeSet<_> = nodes(graph).iter().map(id).collect();
    assert_eq!(
        ids.len(),
        nodes(graph).len(),
        "Duplicate declaration/definition IDs"
    );
    for relation in edges(graph) {
        assert!(ids.contains(relation["source"].as_str().unwrap()));
        assert!(ids.contains(relation["target"].as_str().unwrap()));
    }
    let inputs = graph["inputs"].as_array().unwrap();
    assert!(
        !inputs.is_empty(),
        "Actual compiler/reference provenance is required"
    );
    for input in inputs {
        let path = Path::new(input["path"].as_str().unwrap());
        assert!(path.is_absolute() && path.is_file(), "{input}");
        assert_eq!(
            input["hash"],
            sha(fs::read(path).unwrap()),
            "Dependency provenance differs: {}",
            path.display()
        );
    }
}
fn assert_complete(graph: &Value) {
    assert_eq!(
        graph["analysis_complete"], true,
        "diagnostics={} unresolved={}",
        graph["diagnostics"], graph["unresolved"]
    );
}

#[test]
fn c_cross_translation_units_typedefs_and_type_edges_are_real_compiler_facts() {
    if !backend("c") {
        return;
    }
    let f = Fixture::new("c", false);
    let cache = f.build();
    let graph = &cache["graph"];
    schema_and_provenance(graph, "c");
    assert_complete(graph);
    let process = one(graph, "ledger_process");
    let bump = one(graph, "bump");
    edge(graph, process, bump, "calls");
    assert_eq!(bump["definition"], true);
    assert_eq!(bump["path"], "src/math.c");
    assert_eq!(
        named(graph, "bump").len(),
        1,
        "Header declaration and definition must deduplicate"
    );
    let ledger = *named(graph, "Ledger")
        .iter()
        .find(|node| node["kind"] == "type")
        .expect("C struct Ledger must be a type");
    edge(graph, process, ledger, "uses_type");
    let alias = *named(graph, "Ledger")
        .iter()
        .find(|node| node["kind"] == "alias")
        .unwrap();
    edge(graph, alias, ledger, "aliases");
    assert!(nodes(graph)
        .iter()
        .any(|n| n["name"] == "Count" && n["kind"] == "alias"));
    assert!(f
        .context(id(process), &["--relations", "calls,types"])
        .contains("bump"));
}

#[test]
fn cpp_overloads_virtual_target_alias_and_inheritance_are_precise() {
    if !backend("cpp") {
        return;
    }
    let f = Fixture::new("cpp", false);
    let cache = f.build();
    let graph = &cache["graph"];
    schema_and_provenance(graph, "cpp");
    assert_complete(graph);
    let refunds = named(graph, "warehouse::refund");
    assert_eq!(refunds.len(), 2);
    let integer = *refunds
        .iter()
        .find(|n| n["signature"].as_str().unwrap().contains("int"))
        .unwrap();
    let money = *refunds.iter().find(|n| id(n) != id(integer)).unwrap();
    assert_ne!(id(integer), id(money));
    assert_eq!(integer["definition"], true);
    assert_eq!(money["definition"], true);
    assert_eq!(integer["path"], "src/store.cpp");
    assert_eq!(money["path"], "src/store.cpp");
    let run = one(graph, "warehouse::run");
    edge(graph, run, integer, "calls");
    edge(graph, run, money, "calls");
    edge(graph, money, integer, "calls");
    assert!(
        edges(graph).iter().any(|e| e["source"] == id(run)
            && e["kind"] == "calls"
            && nodes(graph)
                .iter()
                .any(|n| e["target"] == id(n) && n["name"] == "identity")),
        "Concrete identity<Money> call must resolve to its compiler template/specialization target"
    );
    let decoy = one(graph, "telemetry::refund");
    assert!(!edges(graph)
        .iter()
        .any(|e| e["source"] == id(run) && e["target"] == id(decoy) && e["kind"] == "calls"));
    let store = one(graph, "warehouse::Store");
    let discounted = one(graph, "warehouse::DiscountStore");
    edge(graph, discounted, store, "inherits");
    edge(
        graph,
        one(graph, "warehouse::Amount"),
        one(graph, "warehouse::Money"),
        "aliases",
    );
    let reserve = one(graph, "warehouse::reserve");
    let virtual_price = one(graph, "warehouse::Store::price");
    assert_eq!(
        edge(graph, reserve, virtual_price, "calls")["dispatch"],
        "virtual"
    );
    assert!(!edges(graph).iter().any(|e| e["source"] == id(reserve)
        && e["target"] == id(one(graph, "warehouse::DiscountStore::price"))
        && e["kind"] == "calls"));
    edge(graph, run, one(graph, "warehouse::Money"), "uses_type");
    let context = f.context(
        id(run),
        &["--relations", "calls,types", "--budget", "12000"],
    );
    assert!(context.contains("refund"));
    assert!(!context.contains("telemetry::refund"));
    assert!(!context.contains("return Money{stock + allocated}"));
    let mut cards = BTreeSet::new();
    for line in context.lines().filter(|line| line.starts_with("[cpp:")) {
        assert!(
            cards.insert(line.split(']').next().unwrap()),
            "Related symbol contract repeated: {line}"
        );
    }
    assert!(cards.len() >= 3);
    let found = f.okay(&[
        "graph",
        "find",
        "refund",
        "--language",
        "cpp",
        "--budget",
        "12000",
    ]);
    assert!(found.contains("warehouse::refund") && found.contains("telemetry::refund"));
    assert!(!found.contains("return Money{stock + allocated}"));
    let impact = f.okay(&[
        "graph",
        "impact",
        id(integer),
        "--depth",
        "2",
        "--budget",
        "12000",
    ]);
    assert!(impact.contains("warehouse::run"));
    assert!(impact.contains("warehouse::refund"));
    let ambiguous = f.run(&["context", "--symbol", "refund", "--budget", "12000"]);
    assert!(
        !ambiguous.status.success(),
        "Ambiguous names must never pick an overload"
    );
}

#[test]
fn clang_uses_request_source_overlays_instead_of_stale_disk_bodies() {
    if !backend("cpp") {
        return;
    }
    let f = Fixture::new("cpp", false);
    let disk = fs::read_to_string(f.root.join("src/app.cpp")).unwrap();
    let before = f.adapter(&[]);
    assert_complete(&before);
    let altered = disk.replace(
        "Money reserved = refund(8, 2);",
        "Money reserved = Money{10};",
    );
    assert_ne!(altered, disk);
    let after = f.adapter(&[("src/app.cpp", altered)]);
    assert_complete(&after);
    let integer = nodes(&before)
        .iter()
        .find(|n| {
            matches_qualified(n, "warehouse::refund")
                && n["signature"].as_str().unwrap().contains("int")
        })
        .unwrap();
    edge(&before, one(&before, "warehouse::run"), integer, "calls");
    assert!(!edges(&after)
        .iter()
        .any(|e| e["source"] == id(one(&after, "warehouse::run"))
            && e["target"] == id(integer)
            && e["kind"] == "calls"));
    assert_eq!(
        fs::read_to_string(f.root.join("src/app.cpp")).unwrap(),
        disk
    );
}

#[test]
fn csharp_overloads_generic_types_aliases_and_interface_dispatch_are_resolved() {
    if !backend("csharp") {
        return;
    }
    let f = Fixture::new("csharp", false);
    let engine = f.root.join("Engine.cs");
    let engine_source = fs::read_to_string(&engine).unwrap().replace('\n', "\r\n");
    fs::write(&engine, &engine_source).unwrap();
    let cache = f.build();
    let graph = &cache["graph"];
    schema_and_provenance(graph, "csharp");
    assert_complete(graph);
    let refunds = named(graph, "Warehouse.Engine.Refund");
    assert_eq!(refunds.len(), 2);
    let integer = *refunds
        .iter()
        .find(|n| {
            n["signature"].as_str().unwrap().contains("int") || id(n).contains("System.Int32")
        })
        .unwrap();
    let money = *refunds.iter().find(|n| id(n) != id(integer)).unwrap();
    let run = one(graph, "Warehouse.Engine.Run");
    let start = run["start_byte"].as_u64().unwrap() as usize;
    let end = run["end_byte"].as_u64().unwrap() as usize;
    let source = std::str::from_utf8(&engine_source.as_bytes()[start..end]).unwrap();
    assert!(
        source.starts_with("public static Money Run()"),
        "Roslyn offsets must be UTF8 bytes, not UTF16 characters: {source:?}"
    );
    assert!(source.ends_with('}'));
    edge(graph, run, integer, "calls");
    edge(graph, run, money, "calls");
    edge(graph, money, integer, "calls");
    let target = one(graph, "Warehouse.IPricer.Price");
    assert_eq!(
        edge(graph, one(graph, "Warehouse.Engine.Quote"), target, "calls")["dispatch"],
        "virtual"
    );
    edge(
        graph,
        one(graph, "Warehouse.DiscountPricer"),
        one(graph, "Warehouse.BasePricer"),
        "inherits",
    );
    edge(
        graph,
        one(graph, "Warehouse.BasePricer"),
        one(graph, "Warehouse.IPricer"),
        "inherits",
    );
    let amount = nodes(graph)
        .iter()
        .find(|n| n["name"] == "Amount" && n["kind"] == "alias")
        .unwrap();
    edge(graph, amount, one(graph, "Warehouse.Money"), "aliases");
    let saved = one(graph, "Warehouse.Receipt.Saved");
    edge(graph, saved, one(graph, "Warehouse.Money"), "uses_type");
    assert!(edges(graph).iter().any(|e| e["source"] == id(saved)
        && e["kind"] == "uses_type"
        && nodes(graph)
            .iter()
            .any(|n| id(n) == e["target"].as_str().unwrap() && n["name"] == "Box")));
    assert!(
        !f.root.join("msbuild-was-executed.marker").exists(),
        "Target project's MSBuild must not execute"
    );
    assert!(f
        .context(
            id(run),
            &["--relations", "calls,types", "--budget", "12000"]
        )
        .contains("Refund"));
}

#[test]
fn roslyn_uses_supplied_source_trees_and_does_not_execute_msbuild() {
    if !backend("csharp") {
        return;
    }
    let f = Fixture::new("csharp", false);
    let disk = fs::read_to_string(f.root.join("Engine.cs")).unwrap();
    let before = f.adapter(&[]);
    assert_complete(&before);
    let altered = disk.replace("var first = Refund(8, 2);", "var first = new Money(10);");
    assert_ne!(altered, disk);
    let after = f.adapter(&[("Engine.cs", altered)]);
    assert_complete(&after);
    let integer = nodes(&before)
        .iter()
        .find(|n| matches_qualified(n, "Warehouse.Engine.Refund") && id(n).contains("System.Int32"))
        .unwrap();
    edge(
        &before,
        one(&before, "Warehouse.Engine.Run"),
        integer,
        "calls",
    );
    assert!(!edges(&after)
        .iter()
        .any(|e| e["source"] == id(one(&after, "Warehouse.Engine.Run"))
            && e["target"] == id(integer)
            && e["kind"] == "calls"));
    assert_eq!(fs::read_to_string(f.root.join("Engine.cs")).unwrap(), disk);
    assert!(!f.root.join("msbuild-was-executed.marker").exists());
}

#[test]
fn semantic_pages_are_utf8_complete_and_cursors_reject_changed_sources() {
    if !backend("cpp") {
        return;
    }
    let f = Fixture::new("cpp", false);
    let path = f.root.join("src/app.cpp");
    let text = fs::read_to_string(&path).unwrap();
    let comments = (0..80)
        .map(|n| {
            format!("    // 行 {n}: semantic body paging retains every Unicode character 😀\n")
        })
        .collect::<String>();
    let text = text
        .replace("Money run() {", &format!("Money run() {{\n{comments}"))
        .replace(
            "    return identity<Money>",
            "    // 終\n    return identity<Money>",
        )
        .replace('\n', "\r\n");
    fs::write(&path, &text).unwrap();
    let cache = f.build();
    let graph = &cache["graph"];
    assert_complete(graph);
    let run = one(graph, "warehouse::run");
    let start = run["start_byte"].as_u64().unwrap() as usize;
    let end = run["end_byte"].as_u64().unwrap() as usize;
    let bytes = text.as_bytes();
    let selected = std::str::from_utf8(&bytes[start..end]).unwrap();
    assert!(selected.contains("Money run()"));
    assert_eq!(selected.matches('😀').count(), 80);
    assert!(selected.contains("// 終"));
    let budget = 1800;
    let mut page = f.context(
        id(run),
        &["--body", "--relations", "calls,types", "--budget", "1800"],
    );
    let first_cursor = page
        .lines()
        .find_map(|line| line.strip_prefix("continue "))
        .expect("Fixture must require multiple pages")
        .to_owned();
    let mut complete = String::new();
    let mut seen = BTreeSet::new();
    let mut pages = 0;
    loop {
        pages += 1;
        assert!(
            page.len() <= budget,
            "Encoded reply exceeds requested byte budget"
        );
        assert!(pages < 100, "Pagination made no bounded progress");
        let cursor = page
            .lines()
            .find_map(|line| line.strip_prefix("continue "))
            .map(str::to_owned);
        complete.push_str(&page);
        if let Some(cursor) = cursor {
            assert!(seen.insert(cursor.clone()), "Repeated cursor");
            page = f.okay(&["context", "--cursor", &cursor, "--budget", "1800"]);
        } else {
            assert!(page.lines().any(|line| line == "end"));
            break;
        }
    }
    assert!(pages > 1);
    assert_eq!(complete.matches('😀').count(), 80);
    assert_eq!(complete.matches('行').count(), 80);
    assert_eq!(complete.matches('終').count(), 1);
    fs::write(&path, text.replace("Money cash{5}", "Money cash{6}")).unwrap();
    assert!(!f
        .run(&["context", "--cursor", &first_cursor, "--budget", "1800"])
        .status
        .success());
}

#[test]
fn cache_source_keys_ignore_unrelated_docs_but_refresh_real_type_changes() {
    if !backend("cpp") {
        return;
    }
    let f = Fixture::new("cpp", false);
    let before = f.build();
    let raw_id = id(one(&before["graph"], "warehouse::run")).to_owned();
    fs::write(f.root.join("README.md"), "unrelated documentation\n").unwrap();
    let current = Project::open(&f.root).unwrap().snapshot().unwrap();
    let context = f.context(&raw_id, &["--budget", "12000"]);
    assert!(
        context.contains(&current.hash),
        "Semantic output must expose current full apply snapshot"
    );
    assert_eq!(f.cache()["source_key"], before["source_key"]);
    let header = f.root.join("include/warehouse.hpp");
    let text = fs::read_to_string(&header).unwrap();
    fs::write(
        header,
        text.replace(
            "struct Money { int cents; };",
            "struct Money { long cents; };",
        ),
    )
    .unwrap();
    assert!(
        !f.run(&["context", "--symbol", &raw_id]).status.success(),
        "Stale semantic facts must not be served as current"
    );
    f.okay(&["graph", "refresh", "--language", "cpp"]);
    let after = f.cache();
    assert_ne!(after["source_key"], before["source_key"]);
    assert_eq!(id(one(&after["graph"], "warehouse::run")), raw_id);
}

#[test]
fn missing_types_headers_and_unknown_call_targets_stay_explicitly_partial() {
    for language in ["c", "cpp", "csharp"] {
        if !backend(language) {
            continue;
        }
        let f = Fixture::new(language, true);
        let cache = f.build();
        let graph = &cache["graph"];
        schema_and_provenance(graph, language);
        assert_eq!(
            graph["analysis_complete"], false,
            "{language} must not report full coverage"
        );
        assert!(
            !graph["diagnostics"].as_array().unwrap().is_empty()
                || !graph["unresolved"].as_array().unwrap().is_empty()
        );
        if language == "c" || language == "csharp" {
            assert!(
                !graph["unresolved"].as_array().unwrap().is_empty(),
                "Unknown call target must not be guessed"
            );
        }
    }
}

#[test]
fn compile_database_uses_command_directory_and_records_its_actual_bytes() {
    if !backend("cpp") {
        return;
    }
    let f = Fixture::new("cpp", false);
    let directory = f.base.join("compiler cwd");
    fs::create_dir(&directory).unwrap();
    let database = f.base.join("compile_commands.json");
    let entries:Vec<_>=["src/app.cpp","src/store.cpp"].iter().map(|file|json!({
        "directory":directory,"file":f.root.join(file),
        "arguments":["clang++","-std=c++20","-I../project with spaces/include","-c",f.root.join(file),"-o","should-not-be-created.o"]
    })).collect();
    fs::write(&database, serde_json::to_vec(&entries).unwrap()).unwrap();
    f.okay(&[
        "graph",
        "build",
        "--language",
        "cpp",
        "--compile-commands",
        database.to_str().unwrap(),
    ]);
    let cache = f.cache();
    let graph = &cache["graph"];
    schema_and_provenance(graph, "cpp");
    assert_complete(graph);
    assert_eq!(named(graph, "warehouse::refund").len(), 2);
    assert!(graph["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|input| input["path"] == database.to_str().unwrap()
            && input["hash"] == sha(fs::read(&database).unwrap())));
    assert!(
        !directory.join("should-not-be-created.o").exists(),
        "Compile database commands must be parsed, never executed"
    );
}

#[test]
fn external_header_changes_invalidate_cache_without_project_source_changes() {
    if !backend("c") {
        return;
    }
    let f = Fixture::new("c", false);
    let external = f.base.join("external include");
    fs::create_dir(&external).unwrap();
    let header = external.join("external_dependency.h");
    fs::write(
        &header,
        "static inline unsigned external_adjust(unsigned value) { return value + 1u; }\n",
    )
    .unwrap();
    fs::write(f.root.join("src/external.c"),"#include \"external_dependency.h\"\nunsigned external_entry(unsigned value) { return external_adjust(value); }\n").unwrap();
    let include = format!("-I{}", external.display());
    f.okay(&[
        "graph",
        "build",
        "--language",
        "c",
        "--flag",
        "-std=c11",
        "--flag",
        "-Iinclude",
        "--flag",
        &include,
    ]);
    let cache = f.cache();
    let graph = &cache["graph"];
    assert_complete(graph);
    let entry = one(graph, "external_entry");
    let target = one(graph, "external_adjust");
    edge(graph, entry, target, "calls");
    assert_eq!(target["external"], true);
    assert_eq!(target["path"], Value::Null);
    assert!(
        graph["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|input| Path::new(input["path"].as_str().unwrap())
                .canonicalize()
                .unwrap()
                == header.canonicalize().unwrap()
                && input["hash"] == sha(fs::read(&header).unwrap())),
        "Missing real external header provenance: {}",
        graph["inputs"]
    );
    let snapshot = Project::open(&f.root).unwrap().snapshot().unwrap().hash;
    fs::write(
        &header,
        "static inline unsigned external_adjust(unsigned value) { return value + 2u; }\n",
    )
    .unwrap();
    assert_eq!(
        Project::open(&f.root).unwrap().snapshot().unwrap().hash,
        snapshot
    );
    assert!(
        !f.run(&["context", "--symbol", id(entry)]).status.success(),
        "Changed compiler input must stale the semantic index"
    );
    f.okay(&["graph", "refresh", "--language", "c"]);
    let refreshed = f.cache();
    assert!(refreshed["graph"]["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|input| Path::new(input["path"].as_str().unwrap())
            .canonicalize()
            .unwrap()
            == header.canonicalize().unwrap()
            && input["hash"] == sha(fs::read(&header).unwrap())));
}

#[test]
fn unsupported_language_does_not_silently_claim_lexical_semantics() {
    let f = Fixture::new("cpp", false);
    let result = f.run(&["graph", "build", "--language", "unknown-language"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr)
        .to_lowercase()
        .contains("language"));
    assert!(!f
        .root
        .join(".ai-env/semantic/unknown-language.json")
        .exists());
}

fn complete_pages(f: &Fixture, args: &[&str], budget: usize) -> Vec<String> {
    let mut page = f.okay(args);
    let mut pages = Vec::new();
    let mut seen = BTreeSet::new();
    loop {
        assert!(page.len() <= budget);
        assert!(pages.len() < 128);
        let cursor = page
            .lines()
            .find_map(|line| line.strip_prefix("continue "))
            .map(str::to_owned);
        pages.push(page);
        if let Some(cursor) = cursor {
            assert!(seen.insert(cursor.clone()));
            let budget = budget.to_string();
            page = f.okay(&["context", "--cursor", &cursor, "--budget", &budget]);
        } else {
            assert!(pages.last().unwrap().lines().any(|line| line == "end"));
            break;
        }
    }
    pages
}

fn size_case(f: &Fixture, case: &str) -> Value {
    let cache = f.build();
    let graph = &cache["graph"];
    assert_complete(graph);
    let refunds = named(graph, "warehouse::refund");
    let selected = *refunds
        .iter()
        .find(|node| !node["signature"].as_str().unwrap().contains("int"))
        .unwrap();
    let integer = *refunds
        .iter()
        .find(|node| id(node) != id(selected))
        .unwrap();
    edge(graph, selected, integer, "calls");
    edge(graph, one(graph, "warehouse::run"), selected, "calls");
    let snapshot = Project::open(&f.root).unwrap().snapshot().unwrap();
    let files: Vec<_> = snapshot
        .files
        .values()
        .filter(|file| file.path.ends_with(".cpp") || file.path.ends_with(".hpp"))
        .map(|file| json!({"path":file.path,"bytes":file.text.len(),"sha256":file.hash}))
        .collect();
    let source_bytes: usize = files
        .iter()
        .map(|file| file["bytes"].as_u64().unwrap() as usize)
        .sum();
    let relevant_paths = ["include/warehouse.hpp", "src/app.cpp", "src/store.cpp"];
    let relevant_bytes: usize = files
        .iter()
        .filter(|file| relevant_paths.contains(&file["path"].as_str().unwrap()))
        .map(|file| file["bytes"].as_u64().unwrap() as usize)
        .sum();
    let matching: Vec<_> = snapshot
        .files
        .values()
        .flat_map(|file| file.text.split_inclusive('\n'))
        .filter(|line| line.contains("refund"))
        .collect();
    let metric = |pages: &Vec<String>| {
        let bytes = pages.iter().map(String::len).sum::<usize>();
        json!({"pages":pages.len(),"utf8_bytes_including_all_headers_and_cursors":bytes,"full_source_divided_by_reply_bytes":source_bytes as f64/bytes as f64,"relevant_files_divided_by_reply_bytes":relevant_bytes as f64/bytes as f64})
    };
    let mut measurements = serde_json::Map::new();
    for budget in [12000, 1800] {
        let limit = budget.to_string();
        let context = complete_pages(
            f,
            &[
                "context",
                "--symbol",
                id(selected),
                "--relations",
                "calls,callers,types",
                "--depth",
                "1",
                "--budget",
                &limit,
            ],
            budget,
        );
        let body = complete_pages(
            f,
            &[
                "context",
                "--symbol",
                id(selected),
                "--relations",
                "calls,callers,types",
                "--depth",
                "1",
                "--body",
                "--budget",
                &limit,
            ],
            budget,
        );
        let impact = complete_pages(
            f,
            &[
                "graph",
                "impact",
                id(selected),
                "--depth",
                "2",
                "--budget",
                &limit,
            ],
            budget,
        );
        if budget == 12000 {
            let facts = body.join("");
            assert!(facts.contains("warehouse::Money") && facts.contains("warehouse::run"));
            assert!(facts.contains("return refund(stock.cents, allocated.cents);"));
            assert!(impact.join("").contains("warehouse::run"));
        }
        measurements.insert(budget.to_string(),json!({"context_contracts":metric(&context),"context_with_selected_body":metric(&body),"impact_depth2":metric(&impact)}));
    }
    let lexical = complete_pages(f, &["context", "refund", "--budget", "12000"], 12000);
    json!({"case":case,"language":"cpp","analyzer":graph["analyzer"],"source_files":files,"source_file_count":files.len(),"full_raw_source_utf8_bytes":source_bytes,"relevant_raw_source_utf8_bytes":relevant_bytes,"relevant_files":relevant_paths,"lexical_matching_source_line_count":matching.len(),"lexical_matching_source_line_utf8_bytes":matching.iter().map(|line|line.len()).sum::<usize>(),"lexical_context_12000":metric(&lexical),"selector_raw_id":id(selected),"selector_signature":selected["signature"],"measurements":measurements,"model_calls":0,"tokenizer_calls":0,"not_a_token_or_llm_benchmark":true})
}

#[test]
fn semantic_context_size_probe_reports_complete_utf8_bytes_not_model_tokens() {
    if !backend("cpp") {
        return;
    }
    let f = Fixture::new("cpp", false);
    let tiny = size_case(&f, "tiny");
    // Real, independently declared library functions, not inflated comments.
    for module in 0..100 {
        let mut source = format!("namespace unrelated_{module:03} {{\n");
        for function in 0..20 {
            source.push_str(&format!(
                "int transform_{function:02}(int value) {{ return value + {}; }}\n",
                module * 20 + function
            ));
        }
        source.push_str("}\n");
        fs::write(
            f.root.join(format!("src/unrelated_{module:03}.cpp")),
            source,
        )
        .unwrap();
    }
    let scaled = size_case(&f, "100_unrelated_modules_20_functions_each");
    assert_eq!(tiny["selector_raw_id"], scaled["selector_raw_id"]);
    assert_eq!(
        scaled["source_file_count"],
        tiny["source_file_count"].as_u64().unwrap() + 100
    );
    assert_eq!(
        tiny["relevant_raw_source_utf8_bytes"],
        scaled["relevant_raw_source_utf8_bytes"]
    );
    assert_eq!(
        tiny["lexical_matching_source_line_count"],
        scaled["lexical_matching_source_line_count"]
    );
    eprintln!(
        "SEMANTIC_SIZE_PROBE {}",
        json!({"cases":[tiny,scaled],"scope":"Source-selection mechanism only. Ordinary rg-assisted agents also avoid unrelated files. Raw relevant files and lexical projections are separate comparison controls; no equal-information or LLM/token-efficiency inference."})
    );
}

#[test]
fn unavailable_compiler_reports_cause_and_preserves_previous_real_graph() {
    if !backend("cpp") {
        return;
    }
    let f = Fixture::new("cpp", false);
    f.build();
    let cache = f.root.join(".ai-env/semantic/cpp.json");
    let before = fs::read(&cache).unwrap();
    let invalid = f.base.join("invalid-libclang.dylib");
    fs::write(&invalid, "not a compiler shared library").unwrap();
    let result = f.run(&[
        "graph",
        "build",
        "--language",
        "cpp",
        "--flag",
        "-std=c++20",
        "--flag",
        "-Iinclude",
        "--libclang",
        invalid.to_str().unwrap(),
    ]);
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(
        error.contains("no graph published") && error.contains("invalid-libclang.dylib"),
        "{error}"
    );
    assert_eq!(fs::read(cache).unwrap(), before);
}

#[test]
fn primary_template_impact_reaches_instantiated_callers_and_reference_contracts() {
    if !backend("cpp") {
        return;
    }
    let f = Fixture::new("cpp", false);
    fs::write(f.root.join("src/instantiations.cpp"),"template<class T> T convert(T value){return value;}\nint first(){return convert(1);}\ndouble second(){return convert(1.5);}\nint unrelated(){return 3;}\n").unwrap();
    let cache = f.build();
    let graph = &cache["graph"];
    let primary = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "convert" && n["id"].as_str().unwrap().contains("@FT@"))
        .unwrap();
    let primary_id = primary["id"].as_str().unwrap();
    let impact = f.okay(&[
        "graph", "impact", primary_id, "--depth", "2", "--budget", "12000",
    ]);
    for name in ["first", "second"] {
        let caller = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["name"] == name && n["kind"] == "function")
            .unwrap();
        assert!(impact.contains(&aienv::graph::model::public_id(
            "cpp",
            caller["id"].as_str().unwrap()
        )));
    }
    let unrelated = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "unrelated" && n["kind"] == "function")
        .unwrap();
    assert!(!impact.contains(&aienv::graph::model::public_id(
        "cpp",
        unrelated["id"].as_str().unwrap()
    )));
    let specialization = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "convert" && n["id"] != primary["id"])
        .unwrap();
    let references = f.okay(&[
        "context",
        "--symbol",
        specialization["id"].as_str().unwrap(),
        "--relations",
        "references",
        "--budget",
        "12000",
    ]);
    assert!(references.contains(&aienv::graph::model::public_id("cpp", primary_id)));
}
