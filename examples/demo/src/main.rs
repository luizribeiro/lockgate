//! Builds and narrates the end-to-end Lockgate demo, including direct,
//! filesystem, dynamic, and rejected calls.

use anyhow::Result;
use lockgate::{Event, Plan, Runtime, Val};

mod policy;

fn main() -> Result<()> {
    let demo = policy::build()?;
    let (naughty_catalog, naughty_policy) = policy::build_naughty()?;
    let Err(naughty_error) = Plan::new(naughty_catalog, naughty_policy) else {
        anyhow::bail!("naughty unexpectedly produced a valid plan");
    };
    println!();
    println!("[plan] naughty  REFUSED {naughty_error}");

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
    println!("\n[call] caller.run()");
    let values = runtime.call(demo.caller_run, &[])?;
    println!("  => {}", render_values(&values));

    println!("\n[call] filereader.run()");
    let values = runtime.call(demo.filereader_run, &[])?;
    println!("  => {}", render_values(&values));

    println!("\n[call] naughty.run()\n  => unavailable (incomplete policy was refused)");

    println!("\n[call] dynamic.run()");
    let values = runtime.call(demo.dynamic_run, &[])?;
    println!("  => {}", render_values(&values));

    println!("\n[host] still running");
    Ok(())
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
