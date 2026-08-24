#[test]
fn rejects_invalid_unix_projection_shapes() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
