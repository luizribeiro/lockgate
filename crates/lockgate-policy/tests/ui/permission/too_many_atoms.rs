use lockgate_policy::{Needs, Permission};

#[lockgate_policy::capability("documents")]
mod documents {
    use super::Permission;

    pub const LIST: Permission = Permission::new("list");
}

const NEEDS: &[lockgate_policy::Need] = &[documents::LIST.need(); 257];
const TOO_MANY: Needs = Needs::required(NEEDS);

fn main() {}

