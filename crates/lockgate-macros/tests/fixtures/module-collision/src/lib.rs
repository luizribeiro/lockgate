struct Imports;
struct Data;

lockgate::host_bindings!({
    path: "wit",
    world: "fixture",
    imports_type: Imports,
    data: Data,
});
