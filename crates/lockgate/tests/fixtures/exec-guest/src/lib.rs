wit_bindgen::generate!({
    path: "wit",
    world: "fixture",
});

struct Fixture;

impl exports::test::exec::guest::Guest for Fixture {
    fn value() -> u32 {
        42
    }

    async fn suspend() -> u32 {
        test::exec::host::wait().await;
        7
    }
}

export!(Fixture);
