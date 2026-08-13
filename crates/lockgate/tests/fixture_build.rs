mod common;

#[test]
fn builds_exec_guest_component() {
    assert!(common::exec_fixture().starts_with(b"\0asm"));
    assert!(common::exec_concurrent_fixture().starts_with(b"\0asm"));
    assert!(common::exec_fuel_fixture().starts_with(b"\0asm"));
}
