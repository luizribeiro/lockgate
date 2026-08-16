use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use lockgate_plugin::{NoSettings, schemars};

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
fn no_settings_cannot_opt_out_of_closed_policy() {
    let source = r#"
use lockgate_plugin::{MetadataSource, Needs, NoSettings, Plugin, SettingsPolicy, export};

macro_rules! __lockgate_wit_export {
    ($plugin:ident) => {};
}

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
    fs::write(fixture.join("Cargo.toml"), fixture_manifest()).unwrap();
    fs::write(fixture.join("src/lib.rs"), source).unwrap();

    let output = Command::new(env!("CARGO"))
        .args([
            "check",
            "--quiet",
            "--manifest-path",
            path_str(&fixture.join("Cargo.toml")),
            "--target-dir",
            path_str(&target),
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
