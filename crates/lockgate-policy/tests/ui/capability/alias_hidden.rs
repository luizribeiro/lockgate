#[lockgate_policy::capability("vm")]
pub mod vm {
    type Alias = lockgate_policy::Permission;
    pub const READ: Alias = Alias::new("read");
}

fn main() {}
