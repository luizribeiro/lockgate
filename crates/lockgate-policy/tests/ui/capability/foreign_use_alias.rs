mod foreign {
    pub type Alias = lockgate_policy::Permission;
}

#[lockgate_policy::capability("vm")]
pub mod vm {
    use super::foreign::Alias as X;

    pub const READ: X = X::new("read");
}

fn main() {}
