#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use lockgate_plugin::{MetadataSource, Needs};

lockgate_plugin::generate!({
    path: "wit",
    world: "tidy",
});

struct Tidy;

impl lockgate_plugin::Plugin for Tidy {
    const ID: &'static str = "tidy";
    // DISPLAY_NAME, VERSION, and DESCRIPTION come from Cargo package metadata.
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    // Explicit authority claim: this plugin requests no host capabilities.
    const NEEDS: Needs = Needs::NOTHING;
}

impl exports::example::workbench::formatter::Guest for Tidy {
    fn format(text: String) -> String {
        let mut formatted = String::new();
        for word in text.split_whitespace() {
            if !formatted.is_empty() {
                formatted.push(' ');
            }
            formatted.push_str(word);
        }
        formatted
    }
}

impl exports::example::workbench::linter::Guest for Tidy {
    fn lint(text: String) -> Vec<String> {
        let mut findings = Vec::new();
        let mut line = 1;
        let mut previous = None;
        for byte in text.bytes() {
            if byte == b'\n' {
                if matches!(previous, Some(b' ' | b'\t')) {
                    findings.push(format!("line {line} has trailing whitespace"));
                }
                line += 1;
            }
            previous = Some(byte);
        }
        if matches!(previous, Some(b' ' | b'\t')) {
            findings.push(format!("line {line} has trailing whitespace"));
        }
        findings
    }
}

lockgate_plugin::export!(Tidy);
