#![no_std]

extern crate alloc;

use alloc::string::String;
use lockgate_plugin::{MetadataSource, Needs, NoSettings};

lockgate_plugin::generate!({
    path: "../../data/detached_jobs",
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "detached-jobs";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

impl exports::test::detached_jobs::guest::Guest for Fixture {
    fn start(kind: u8) -> String {
        match test::detached_jobs::application::start(kind) {
            Ok(job_id) => job_id,
            Err(error) => alloc::format!("error:{error}"),
        }
    }

    async fn cancel() {
        if test::detached_jobs::application::start(0).is_err() {
            core::arch::wasm32::unreachable();
        }
        test::detached_jobs::application::suspend().await;
    }

    fn healthy() -> u32 {
        7
    }
}

lockgate_plugin::export!(Fixture);
