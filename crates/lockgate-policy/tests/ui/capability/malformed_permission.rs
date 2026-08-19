#[lockgate_policy::capability("vm")]
pub mod vm {
    pub const READ: lockgate_policy::Permission = lockgate_policy::Permission::new("Read_All");
}

fn main() {}
