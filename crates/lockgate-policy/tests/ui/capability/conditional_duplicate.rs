use lockgate_policy::Permission;

#[lockgate_policy::capability("vm")]
pub mod vm {
    use super::Permission;

    #[cfg(any())]
    pub const FIRST: Permission = Permission::new("read");
    #[cfg(not(any()))]
    pub const SECOND: Permission = Permission::new("read");
    #[cfg(not(any()))]
    pub const THIRD: Permission = Permission::new("read");
}

fn main() {}
