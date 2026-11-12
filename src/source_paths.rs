//! Shared path conventions for source navigation, never test execution proof.
use std::path::Path;

pub(crate) fn test_path(path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    path.split('/').any(|part| {
        matches!(
            part.to_ascii_lowercase().as_str(),
            "test" | "tests" | "__tests__"
        ) || part.to_ascii_lowercase().ends_with(".tests")
    }) || name.starts_with("test_")
        || name.starts_with("tests.")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains("_test.")
        || name.contains("_tests.")
        || name.ends_with("Tests.cs")
        || name.ends_with("Test.cs")
}
