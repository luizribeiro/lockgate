#[derive(Clone)]
struct CallData;

#[derive(Clone)]
struct Imports;

lg::host_bindings!({
    path: "../../data/host_bindings",
    world: "fixture",
    imports_type: Imports,
    data: CallData,
});

#[lg::guarded]
impl application::Host for Imports {
    #[lg::no_capability_required(reason = "compile-only renamed-facade fixture")]
    async fn read_data(&mut self, _cx: lg::HostCtx<'_, CallData>) -> String {
        String::new()
    }

    #[lg::no_capability_required(reason = "compile-only renamed-facade fixture")]
    async fn caller(&mut self, _cx: lg::HostCtx<'_, CallData>) -> String {
        String::new()
    }

    #[lg::no_capability_required(reason = "compile-only renamed-facade fixture")]
    async fn transform(
        &mut self,
        _cx: lg::HostCtx<'_, CallData>,
        request: application::Request,
    ) -> Result<application::Request, String> {
        Ok(request)
    }

    #[lg::no_capability_required(reason = "compile-only renamed-facade fixture")]
    async fn first(&mut self, _cx: lg::HostCtx<'_, CallData>) {}

    #[lg::no_capability_required(reason = "compile-only renamed-facade fixture")]
    async fn second(&mut self, _cx: lg::HostCtx<'_, CallData>) {}

    #[lg::no_capability_required(reason = "compile-only renamed-facade fixture")]
    async fn startup(&mut self, _cx: lg::HostCtx<'_, CallData>) -> u32 {
        0
    }
}
