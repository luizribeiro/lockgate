#[derive(Clone)]
struct CallData;

#[derive(Clone)]
struct Imports;

lg::host_bindings!({
    path: "../../data/host_bindings",
    world: "fixture",
    imports: Imports,
    data: CallData,
});

impl application::Host for Imports {
    async fn read_data(&mut self, _cx: lg::HostCtx<'_, CallData>) -> String {
        String::new()
    }

    async fn caller(&mut self, _cx: lg::HostCtx<'_, CallData>) -> String {
        String::new()
    }

    async fn transform(
        &mut self,
        _cx: lg::HostCtx<'_, CallData>,
        request: application::Request,
    ) -> Result<application::Request, String> {
        Ok(request)
    }

    async fn first(&mut self, _cx: lg::HostCtx<'_, CallData>) {}

    async fn second(&mut self, _cx: lg::HostCtx<'_, CallData>) {}

    async fn startup(&mut self, _cx: lg::HostCtx<'_, CallData>) -> u32 {
        0
    }
}
