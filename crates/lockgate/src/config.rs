//! Framework-owned configuration bindings.

wasmtime::component::bindgen!({
    path: "wit",
    world: "plugin",
    imports: { default: async },
    exports: { default: async },
});

#[cfg(test)]
mod tests {
    use wit_parser::Resolve;

    #[test]
    fn owned_config_package_has_the_framework_contract() {
        let mut resolve = Resolve::new();
        let (package, _) = resolve
            .push_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/wit"))
            .unwrap();
        let world = resolve.select_world(&[package], Some("plugin")).unwrap();
        let world = &resolve.worlds[world];

        assert_eq!(world.imports.len(), 1);
        assert_eq!(world.exports.len(), 1);
        assert!(
            world
                .imports
                .keys()
                .any(|name| resolve.name_world_key(name) == "lockgate:config/settings")
        );
        assert!(
            world
                .exports
                .keys()
                .any(|name| resolve.name_world_key(name) == "lockgate:config/schema")
        );
    }
}
