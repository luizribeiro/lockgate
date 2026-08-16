use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn metadata_source_failures_have_field_specific_diagnostics() {
    check_compile_failure(
        "missing-description",
        r#"
use lockgate_plugin::{MetadataSource, Needs, NoSettings, Plugin, export};

macro_rules! __lockgate_wit_export {
    ($plugin:ident) => {};
}

struct MissingDescription;

impl Plugin for MissingDescription {
    const ID: &'static str = "missing-description";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("Missing description");
    const VERSION: MetadataSource = MetadataSource::Explicit("1.0");
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

export!(MissingDescription);
"#,
        "Lockgate plugin Cargo description is missing or empty; set description in Cargo.toml, or declare MetadataSource::Explicit(...) or MetadataSource::Absent",
    );
    check_compile_failure(
        "absent-name",
        r#"
use lockgate_plugin::{MetadataSource, Needs, NoSettings, Plugin, export};

macro_rules! __lockgate_wit_export {
    ($plugin:ident) => {};
}

struct AbsentName;

impl Plugin for AbsentName {
    const ID: &'static str = "absent-name";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Absent;
    const VERSION: MetadataSource = MetadataSource::Explicit("1.0");
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

export!(AbsentName);
"#,
        "Lockgate plugin display name cannot use MetadataSource::Absent because name is required by the wire format",
    );
    check_compile_failure(
        "absent-version",
        r#"
use lockgate_plugin::{MetadataSource, Needs, NoSettings, Plugin, export};

macro_rules! __lockgate_wit_export {
    ($plugin:ident) => {};
}

struct AbsentVersion;

impl Plugin for AbsentVersion {
    const ID: &'static str = "absent-version";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("Absent version");
    const VERSION: MetadataSource = MetadataSource::Absent;
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

export!(AbsentVersion);
"#,
        "Lockgate plugin version cannot use MetadataSource::Absent because version is required by the wire format",
    );
}

fn check_compile_failure(case: &str, source: &str, expected: &str) {
    let fixture = fixture_dir(case);
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
        .expect("failed to check metadata diagnostic fixture");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "{case} unexpectedly compiled");
    assert!(
        stderr.contains(expected),
        "{case} did not emit the expected diagnostic\nstderr:\n{stderr}"
    );

    fs::remove_dir_all(&fixture).unwrap();
}

fn fixture_dir(case: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/metadata-source-diagnostics")
        .join(case)
}

fn fixture_manifest() -> String {
    format!(
        r#"[package]
name = "metadata-source-diagnostic"
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
