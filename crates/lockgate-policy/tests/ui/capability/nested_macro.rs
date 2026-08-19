use lockgate_policy::Permission;

#[lockgate_policy::capability("vm")]
pub mod vm {
    use super::Permission;

    macro_rules! declare_read {
        () => {
            pub const READ: Permission = Permission::new("read");
        };
    }

    declare_read!();
}

fn main() {}
