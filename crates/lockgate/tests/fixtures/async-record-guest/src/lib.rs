#![no_std]

extern crate alloc;

use alloc::{format, string::String, vec};
use lockgate_plugin::{MetadataSource, Needs, NoSettings};

lockgate_plugin::generate!({
    path: "wit",
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "async-record";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

impl exports::test::async_record::guest::Guest for Fixture {
    async fn run(
        req: exports::test::async_record::guest::Request,
    ) -> Result<exports::test::async_record::guest::Reply, String> {
        use exports::test::async_record::guest::{Piece, Reply};

        if req.prompt.is_empty() {
            return Err("prompt must not be empty".into());
        }

        Ok(Reply {
            pieces: vec![Piece::Text(req.prompt), Piece::Stop],
            model: format!("fixture-{}", req.limit),
        })
    }
}

lockgate_plugin::export!(Fixture);
