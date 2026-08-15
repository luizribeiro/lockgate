use std::path::Path;

const VENDORED_CONTRACTS: &[&str] = &[
    "plugins/tidy/wit/deps/workbench/world.wit",
    "plugins/counter/wit/deps/workbench/world.wit",
];

#[test]
fn vendored_contracts_match_the_host_contract() {
    let workbench = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workbench host should have a parent directory");
    let host_contract = workbench.join("wit/world.wit");
    let host_bytes = read(&host_contract);

    for relative in VENDORED_CONTRACTS {
        let vendored_contract = workbench.join(relative);
        let vendored_bytes = read(&vendored_contract);
        assert_eq!(
            vendored_bytes,
            host_bytes,
            "vendored contract `{}` differs from host contract `{}`; re-vendor the copy",
            vendored_contract.display(),
            host_contract.display(),
        );
    }
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path)
        .unwrap_or_else(|error| panic!("could not read contract `{}`: {error}", path.display()))
}
