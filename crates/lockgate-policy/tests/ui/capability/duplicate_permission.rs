#[lockgate_policy::capability("vm")]
pub mod vm {
    pub const READ: lockgate_policy::Permission = lockgate_policy::Permission::new("read");
    pub const READ_AGAIN: lockgate_policy::Permission =
        lockgate_policy::Permission::new("read");
}

fn main() {}
