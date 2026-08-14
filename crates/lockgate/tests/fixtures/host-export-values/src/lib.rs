lg::host_bindings!({
    path: "../../data/host_export_values",
    world: "fixture",
});

#[allow(dead_code)]
fn keyword_interface_is_sanitized() {
    let _: Option<type_::Role> = None;
}
