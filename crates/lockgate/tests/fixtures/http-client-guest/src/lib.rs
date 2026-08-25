use core::time::Duration;
use lockgate_http::Client;
use lockgate_plugin::{
    Deserialize, JsonSchema, MetadataSource, Need, Needs, Plugin, ScopeRef, net,
};

lockgate_plugin::generate!({
    path: "wit",
    world: "fixture",
});

const REQUIRED: &[Need] = &[net::EGRESS.need(&[ScopeRef::setting("/origin")])];

#[derive(Deserialize, JsonSchema)]
#[schemars(crate = "lockgate_plugin::schemars")]
#[serde(
    crate = "lockgate_plugin::serde",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
struct Settings {
    origin: String,
}

struct Fixture;

impl Plugin for Fixture {
    const ID: &'static str = "http-client";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::required(REQUIRED);
    type Settings = Settings;
}

impl exports::test::http_client::guest::Guest for Fixture {
    async fn get(url: String) -> Result<exports::test::http_client::guest::Response, String> {
        get(Client::new(), url).await
    }

    async fn get_with_first_byte_timeout(
        url: String,
        timeout_millis: u64,
    ) -> Result<exports::test::http_client::guest::Response, String> {
        get(
            Client::new().with_first_byte_timeout(Duration::from_millis(timeout_millis)),
            url,
        )
        .await
    }

    async fn get_with_timeouts(
        url: String,
        timeout_millis: u64,
    ) -> Result<exports::test::http_client::guest::Response, String> {
        let timeout = Duration::from_millis(timeout_millis);
        get(
            Client::new()
                .with_connect_timeout(timeout)
                .with_first_byte_timeout(timeout)
                .with_between_bytes_timeout(timeout),
            url,
        )
        .await
    }
}

async fn get(
    client: Client,
    url: String,
) -> Result<exports::test::http_client::guest::Response, String> {
    let settings = Fixture::settings();
    let _ = settings.origin;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().map_err(|error| error.to_string())?;
    Ok(exports::test::http_client::guest::Response { status, body })
}

lockgate_plugin::export!(Fixture);
