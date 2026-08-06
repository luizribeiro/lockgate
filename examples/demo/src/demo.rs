//! Narrates the valid direct, filesystem, and dynamic calls plus naughty's rejected plan.

use crate::policy;
use anyhow::Result;
use lockgate::{Event, Runtime, Val};
use std::path::Path;

pub(crate) fn run(root: &Path) -> Result<()> {
    let root = root.canonicalize()?;
    let demo = policy::build(&root)?;
    println!();
    println!("[plan] naughty  REFUSED {}", policy::naughty_error(&root)?);

    let runtime = Runtime::with_observer(demo.plan, |event| match event {
        Event::DirectCall { target } => println!("  [direct] -> {target}"),
        Event::DynamicLookup {
            caller,
            target,
            allowed,
        } => println!(
            "  [dynamic] {caller} -> {target}  {}",
            if allowed { "ALLOWED" } else { "DENIED" }
        ),
    })?;
    run_and_print(&runtime, demo.caller_run, "caller.run()");
    run_and_print(&runtime, demo.filereader_run, "filereader.run()");
    println!("\n[call] naughty.run()\n  => unavailable (incomplete policy was refused)");
    run_and_print(&runtime, demo.dynamic_run, "dynamic.run()");
    println!("\n[host] still running");
    Ok(())
}

fn run_and_print(runtime: &Runtime, export: lockgate::ExportId, label: &str) {
    println!("\n[call] {label}");
    match runtime.call(export, &[]) {
        Ok(values) => println!("  => {}", render_values(&values)),
        Err(error) => println!("  => ERROR {error:#}"),
    }
}

fn render_values(values: &[Val]) -> String {
    match values {
        [Val::Result(Ok(Some(value)))] => match value.as_ref() {
            Val::String(value) => format!("{value:?}"),
            other => format!("{other:?}"),
        },
        [Val::List(values)] => format!("{values:?}"),
        other => format!("{other:?}"),
    }
}
