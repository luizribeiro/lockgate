#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use lockgate_plugin::{MetadataSource, Needs, NoSettings};

lockgate_plugin::generate!({
    path: "../../data/resource_guarded",
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "resource-guarded";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

impl exports::test::resource_guarded::guest::Guest for Fixture {
    fn live_send() -> String {
        let session = test::resource_guarded::sessions::Session::new(true);
        render(session.send("hello"))
    }

    fn repeated_call() -> String {
        let session = test::resource_guarded::sessions::Session::new(true);
        let first = render(session.send("first"));
        session
            .set_current(false)
            .expect("fixture membership mutation must succeed");
        let second = render(session.send("second"));
        alloc::format!("{first},{second}")
    }

    fn stale_handle() -> String {
        let session = test::resource_guarded::sessions::Session::new(true);
        session
            .invalidate()
            .expect("fixture invalidation must succeed");
        let replacement = test::resource_guarded::sessions::Session::new(true);
        let result = render(session.send("stale"));
        drop(replacement);
        result
    }

    fn acquire_without_authority() -> String {
        let session = test::resource_guarded::sessions::Session::new(true);
        render(session.send("ungranted"))
    }

    fn checked_resource() -> String {
        let allowed = test::resource_guarded::sessions::Session::new(true);
        let denied = test::resource_guarded::sessions::Session::new(false);
        let allowed = render(allowed.checked_send("hello"));
        let denied = render(denied.checked_send("secret"));
        alloc::format!("{allowed},{denied}")
    }
}

fn render(result: Result<String, test::resource_guarded::sessions::SessionError>) -> String {
    match result {
        Ok(value) => alloc::format!("ok:{value}"),
        Err(test::resource_guarded::sessions::SessionError::NotFound) => "not-found".to_string(),
        Err(test::resource_guarded::sessions::SessionError::Denied) => "denied".to_string(),
    }
}

lockgate_plugin::export!(Fixture);
