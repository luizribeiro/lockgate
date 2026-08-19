#[lockgate_policy::capability("vm")]
pub mod vm {
    use lockgate_policy::Permission as X;

    pub const READ: X = X::new("read");
}

fn main() {}
