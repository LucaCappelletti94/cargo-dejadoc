//! An integration test target with two copies.

#[test]
fn renders_values() {
    let out = fnws::render::Alpha.render(&[1, 2, 3]);
    assert_eq!(out, "1,2,3");
    assert!(out.contains(','));
    assert_eq!(out.len(), 5);
    assert!(!out.is_empty());
}

#[test]
fn renders_values_again() {
    let out = fnws::render::Alpha.render(&[1, 2, 3]);
    assert_eq!(out, "1,2,3");
    assert!(out.contains(','));
    assert_eq!(out.len(), 5);
    assert!(!out.is_empty());
}
