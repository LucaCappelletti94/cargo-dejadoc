//! An integration test target with two copies.

#[test]
fn renders_values() {
    let out = fnws::render::Alpha.render(&[1, 2, 3]);
    assert_eq!(out, "1,2,3");
    assert!(out.contains(','));
}

#[test]
fn renders_values_again() {
    let out = fnws::render::Alpha.render(&[1, 2, 3]);
    assert_eq!(out, "1,2,3");
    assert!(out.contains(','));
}
