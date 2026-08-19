use lockgate_policy::Permission;

fn requires_qualified(_: Permission) {}

fn main() {
    requires_qualified(Permission::new("read"));
}

