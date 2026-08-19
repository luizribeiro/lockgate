use lockgate_policy::{Needs, Permission};

#[lockgate_policy::capability("documents")]
mod documents {
    use super::Permission;

    pub const LIST: Permission = Permission::new("list");
}

const DUPLICATE: Needs =
    Needs::required(&[documents::LIST.need(), documents::LIST.need()]);

fn main() {}

