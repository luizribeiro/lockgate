#[test]
fn acceptance_and_effective_grants_have_no_public_construction_path() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/effective_grants/*.rs");
}
