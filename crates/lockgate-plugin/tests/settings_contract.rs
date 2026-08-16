use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use lockgate_plugin::{
    Deserialize, JsonSchema, MetadataSource, Needs, NoSettings, Plugin, SettingsPolicy, schemars,
};

#[derive(Deserialize, JsonSchema)]
#[schemars(crate = "lockgate_plugin::schemars")]
#[allow(dead_code)]
struct DocumentedSettings {
    /// Text shown before the configured name.
    prefix: String,
    count: u32,
}

struct ClosedPlugin;

impl Plugin for ClosedPlugin {
    const ID: &'static str = "closed";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("Closed");
    const VERSION: MetadataSource = MetadataSource::Explicit("1.0");
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = DocumentedSettings;
}

struct OpenPlugin;

impl Plugin for OpenPlugin {
    const ID: &'static str = "open";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("Open");
    const VERSION: MetadataSource = MetadataSource::Explicit("1.0");
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    const SETTINGS_POLICY: SettingsPolicy = SettingsPolicy::Open;
    type Settings = DocumentedSettings;
}

#[test]
fn no_settings_accepts_only_an_empty_object() {
    assert_eq!(
        serde_json::from_str::<NoSettings>("{}").unwrap(),
        NoSettings
    );

    let error = serde_json::from_str::<NoSettings>(r#"{"surprise": true}"#).unwrap_err();
    assert!(error.to_string().contains("surprise"), "{error}");
}

#[test]
fn no_settings_schema_is_a_closed_empty_object() {
    let schema = schemars::SchemaGenerator::default().into_root_schema_for::<NoSettings>();
    let value = serde_json::to_value(schema).unwrap();

    assert_eq!(value["type"], "object");
    assert_eq!(value["properties"], serde_json::json!({}));
    assert_eq!(value["unevaluatedProperties"], false);
}

#[test]
fn generated_schema_uses_deserialize_shape_and_closes_the_top_level() {
    let schema: serde_json::Value =
        serde_json::from_str(&lockgate_plugin::__private::settings_schema::<ClosedPlugin>())
            .unwrap();

    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["required"], serde_json::json!(["prefix", "count"]));
    assert_eq!(schema["unevaluatedProperties"], false);
    assert_eq!(
        schema["properties"]["prefix"]["description"],
        "Text shown before the configured name."
    );
}

#[test]
fn open_policy_is_an_explicit_top_level_opt_out() {
    let schema: serde_json::Value =
        serde_json::from_str(&lockgate_plugin::__private::settings_schema::<OpenPlugin>()).unwrap();

    assert!(schema.get("unevaluatedProperties").is_none());
}

#[test]
fn no_settings_cannot_opt_out_of_closed_policy() {
    let source = r#"
use lockgate_plugin::{MetadataSource, Needs, NoSettings, Plugin, SettingsPolicy, export};

lockgate_plugin::generate!({ path: "wit", world: "fixture" });

struct Invalid;

impl Plugin for Invalid {
    const ID: &'static str = "invalid";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("Invalid");
    const VERSION: MetadataSource = MetadataSource::Explicit("1.0");
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    const SETTINGS_POLICY: SettingsPolicy = SettingsPolicy::Open;
    type Settings = NoSettings;
}

export!(Invalid);
"#;
    check_compile_failure(
        "no-settings-open",
        source,
        "Lockgate NoSettings cannot use SettingsPolicy::Open",
    );
}

fn check_compile_failure(case: &str, source: &str, expected: &str) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/settings-contract-diagnostics")
        .join(case);
    let target = fixture.join("target");
    let _ = fs::remove_dir_all(&fixture);
    fs::create_dir_all(fixture.join("src")).unwrap();
    fs::create_dir_all(fixture.join("wit")).unwrap();
    fs::write(fixture.join("Cargo.toml"), fixture_manifest()).unwrap();
    fs::write(fixture.join("src/lib.rs"), source).unwrap();
    fs::write(
        fixture.join("wit/world.wit"),
        "package test:settings-contract; world fixture {}",
    )
    .unwrap();

    let output = Command::new(env!("CARGO"))
        .args([
            "check",
            "--quiet",
            "--manifest-path",
            path_str(&fixture.join("Cargo.toml")),
            "--target-dir",
            path_str(&target),
            "--target",
            "wasm32-wasip2",
        ])
        .output()
        .expect("failed to check settings contract fixture");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "{case} unexpectedly compiled");
    assert!(stderr.contains(expected), "unexpected stderr:\n{stderr}");

    fs::remove_dir_all(&fixture).unwrap();
}

fn fixture_manifest() -> String {
    format!(
        r#"[package]
name = "settings-contract-diagnostic"
version = "0.1.0"
edition = "2024"

[dependencies]
lockgate-plugin = {{ path = {:?}, default-features = false }}

[workspace]
"#,
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    )
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("fixture path must be valid UTF-8")
}
