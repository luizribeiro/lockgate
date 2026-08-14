#![no_std]

lockgate_plugin::generate!({
    path: "wit",
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "exec-concurrent";
}

impl exports::test::exec_concurrent::guest::Guest for Fixture {
    async fn run() -> u32 {
        futures::join!(
            test::exec_concurrent::host::first(),
            test::exec_concurrent::host::second(),
        );
        2
    }
}

lockgate_plugin::export!(Fixture);
