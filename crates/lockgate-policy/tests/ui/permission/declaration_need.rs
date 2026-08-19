use lockgate_policy::Permission;

fn main() {
    Permission::new("read").need();
}

