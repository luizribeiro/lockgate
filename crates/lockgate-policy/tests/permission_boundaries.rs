#[test]
fn permission_authoring_boundaries_are_teaching_diagnostics() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/permission/*.rs");
}
