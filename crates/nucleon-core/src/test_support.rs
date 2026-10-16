//! Isolation for tests that exercise public APIs using the global support path.

/// Returns true in the parent after running this test in a private subprocess.
/// The child returns false and executes the test body with an isolated support path.
pub(crate) fn run_in_isolated_support(test: &str) -> bool {
    const CHILD_TEST: &str = "NUCLEON_ISOLATED_SUPPORT_TEST";
    if std::env::var(CHILD_TEST).as_deref() == Ok(test) {
        return false;
    }
    let support = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env_clear()
        .env(CHILD_TEST, test)
        .env("NUCLEON_SUPPORT_DIR", support.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed;"),
        "Isolated test {test} failed ({status}):\n{stdout}\n{stderr}",
        status = output.status,
        stderr = String::from_utf8_lossy(&output.stderr)
    );
    true
}
