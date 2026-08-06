use crate::{
    plugin::{self, print_load},
    runtime::{PluginTable, call_runner, instantiate, render_values},
};
use anyhow::Result;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use wasmtime::{Config, Engine};

const IDS: [&str; 5] = ["greeter", "caller", "filereader", "naughty", "dynamic"];

pub(crate) fn run() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()?;
    let mut config = Config::new();
    config.wasm_component_model(true).consume_fuel(true);
    let engine = Engine::new(&config)?;
    let plugins = Arc::new(Mutex::new(PluginTable::default()));
    let mut definitions = Vec::new();

    for id in IDS {
        match plugin::load(&engine, &root, id) {
            Ok(definition) => {
                print_load(&definition);
                plugins
                    .lock()
                    .unwrap()
                    .targets
                    .extend(definition.targets.clone());
                definitions.push(definition);
            }
            Err(error) => println!("[load] {id:<12} REFUSED {error:#}"),
        }
    }
    println!();

    for definition in definitions {
        let id = definition.manifest.id.clone();
        match instantiate(&root, &plugins, &engine, definition) {
            Ok(runtime) => {
                plugins
                    .lock()
                    .unwrap()
                    .runtimes
                    .insert(id, Arc::new(Mutex::new(runtime)));
            }
            Err(error) => println!("[instantiate] {id:<8} REFUSED {error:#}"),
        }
    }

    run_and_print(
        &plugins,
        "caller",
        "demo:caller/runner@0.1.0",
        "caller.run()",
    );
    run_and_print(
        &plugins,
        "filereader",
        "demo:filereader/runner@0.1.0",
        "filereader.run()",
    );
    println!("\n[call] naughty.run()\n  => unavailable (undeclared direct import was refused)");
    run_and_print(
        &plugins,
        "dynamic",
        "demo:dynamic/runner@0.1.0",
        "dynamic.run()",
    );
    println!("\n[host] still running");
    Ok(())
}

fn run_and_print(plugins: &Arc<Mutex<PluginTable>>, id: &str, interface: &str, label: &str) {
    println!("\n[call] {label}");
    match call_runner(plugins, id, interface) {
        Ok(values) => println!("  => {}", render_values(&values)),
        Err(error) => println!("  => ERROR {error:#}"),
    }
}
