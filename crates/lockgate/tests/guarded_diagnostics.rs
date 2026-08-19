#[test]
fn guarded_classification_errors_are_teaching_diagnostics() {
    let cases = trybuild::TestCases::new();
    for case in [
        "control_flow_target",
        "denial_error_not_convertible",
        "destructured_target",
        "double_classification",
        "empty_reason",
        "missing_classification",
        "missing_reason",
        "missing_target_on_scoped",
        "no_matching_resolver",
        "resolver_error_not_convertible",
        "resource_handle_path",
        "self_target",
        "target_on_unscoped",
        "wrong_scoped_resource",
    ] {
        cases.compile_fail(format!("tests/ui/guarded/{case}.rs"));
    }
    cases.pass("tests/ui/guarded/hygienic_identifiers.rs");
}
