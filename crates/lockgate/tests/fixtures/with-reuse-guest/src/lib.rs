#![no_std]

extern crate self as with_reuse_guest;

use lockgate_plugin::{MetadataSource, Needs, NoSettings};

pub mod shared_types {
    mod bindings {
        use lockgate_plugin::__wit_bindgen as wit_bindgen;

        lockgate_plugin::__wit_bindgen::generate!({
            inline: r#"
                package test:with-reuse@1.2.3;

                interface types {
                    record request {
                        value: u32,
                    }
                }

                interface guest {
                    use types.{request};
                    round-trip: func(request: request) -> request;
                }

                world sdk-types {
                    import types;
                }

                world fixture {
                    export types;
                    export guest;
                }
            "#,
            world: "sdk-types",
        });
    }

    pub use bindings::test::with_reuse::types::*;
}

lockgate_plugin::generate!({
    inline: r#"
        package test:with-reuse@1.2.3;

        interface types {
            record request {
                value: u32,
            }
        }

        interface guest {
            use types.{request};
            round-trip: func(request: request) -> request;
        }

        world sdk-types {
            import types;
        }

        world fixture {
            import types;
            export guest;
        }
    "#,
    world: "fixture",
    with: {
        "test:with-reuse/types@1.2.3": ::with_reuse_guest::shared_types,
    },
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "with-reuse";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

impl exports::test::with_reuse::guest::Guest for Fixture {
    fn round_trip(request: shared_types::Request) -> shared_types::Request {
        request
    }
}

lockgate_plugin::export!(Fixture);
