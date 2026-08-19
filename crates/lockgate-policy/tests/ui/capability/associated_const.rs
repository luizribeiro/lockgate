use lockgate_policy::Permission;

#[lockgate_policy::capability("vm")]
pub mod vm {
    use super::Permission;

    pub struct Marker;

    impl Marker {
        pub const READ: Permission = Permission::new("read");
    }
}

fn main() {}
