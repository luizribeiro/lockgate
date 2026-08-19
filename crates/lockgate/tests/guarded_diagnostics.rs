#[test]
fn guarded_classification_errors_are_teaching_diagnostics() {
    let cases = trybuild::TestCases::new();
    for case in [
        "control_flow_target",
        "destructured_target",
        "double_classification",
        "empty_reason",
        "missing_classification",
        "missing_reason",
        "missing_target_on_scoped",
        "self_target",
        "target_on_unscoped",
    ] {
        cases.compile_fail(format!("tests/ui/guarded/{case}.rs"));
    }
}
