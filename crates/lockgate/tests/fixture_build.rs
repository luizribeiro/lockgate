mod common;

#[test]
fn builds_exec_guest_component() {
    assert!(common::EXEC_FIXTURE.starts_with(b"\0asm"));
    assert!(common::EXEC_CONCURRENT_FIXTURE.starts_with(b"\0asm"));
    assert!(common::EXEC_FUEL_FIXTURE.starts_with(b"\0asm"));
    assert!(common::PUBLIC_FIXTURE.starts_with(b"\0asm"));
}
