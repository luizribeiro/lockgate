#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use lockgate_plugin::{MetadataSource, Needs, NoSettings};

lockgate_plugin::generate!({
    path: "../../data/guarded_bindings",
    world: "vm-runtime",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "guarded-wiring";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

impl exports::test::guarded::guest::Guest for Fixture {
    fn create(pool: String) -> String {
        render(test::guarded::vm::create(&pool))
    }

    fn exec(vm: String, command: String) -> String {
        render(test::guarded::vm::exec(&vm, &command))
    }

    fn destroy(vm: String) -> String {
        match test::guarded::vm::destroy(&vm) {
            Ok(()) => "ok".to_string(),
            Err(error) => render_error(error),
        }
    }

    fn list_pools() -> String {
        match test::guarded::vm::list_pools() {
            Ok(pools) => pools.join(","),
            Err(error) => render_error(error),
        }
    }

    fn protocol_version() -> String {
        test::guarded::vm::protocol_version()
    }
}

fn render(result: Result<String, test::guarded::vm::VmError>) -> String {
    match result {
        Ok(value) => alloc::format!("ok:{value}"),
        Err(error) => render_error(error),
    }
}

fn render_error(error: test::guarded::vm::VmError) -> String {
    match error {
        test::guarded::vm::VmError::NotFound => "not-found".to_string(),
        test::guarded::vm::VmError::Denied => "denied".to_string(),
    }
}

lockgate_plugin::export!(Fixture);
