//! Embed the contract the transform enforces, and refuse to build with one
//! it could not enforce exactly: the load, lint and compile `covenant gate`
//! runs, done here, so a broken contract fails `cargo build` rather than a
//! broker.
//!
//! - `COVENANT_CONTRACT` (required): the contract, absolute or relative to
//!   this directory; `covenant: 1` or ODCS v3.
//! - `COVENANT_MODEL`: the model, when the contract has more than one.
//! - `COVENANT_NO_UNIQUE=1`: build even though the model declares `unique`
//!   fields. A transform keeps its state per partition and loses it on
//!   restart, so it cannot hold `unique` across a topic; without this the
//!   build refuses such a model, as `covenant gate` without `--no-unique`
//!   would not skip it.

use std::path::{Path, PathBuf};

use covenant::prelude::{CompiledContract, Contract};

fn main() {
    for var in ["COVENANT_CONTRACT", "COVENANT_MODEL", "COVENANT_NO_UNIQUE"] {
        println!("cargo::rerun-if-env-changed={var}");
    }
    if let Err(message) = embed() {
        eprintln!("\nerror: {message}\n");
        std::process::exit(1);
    }
    // A transform instance gets 2 MiB of memory by default, and Rust's 1 MiB
    // WebAssembly stack would take half of it; the gate needs far less. Set
    // here, not in RUSTFLAGS, which a CI's own RUSTFLAGS would replace.
    if std::env::var("CARGO_CFG_TARGET_FAMILY").is_ok_and(|f| f.split(',').any(|f| f == "wasm")) {
        println!("cargo::rustc-link-arg-bins=-zstack-size=262144");
    }
}

fn embed() -> Result<(), String> {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let path = std::env::var_os("COVENANT_CONTRACT")
        .map(|p| dir.join(p))
        .ok_or("set COVENANT_CONTRACT to the contract this transform enforces")?;
    println!("cargo::rerun-if-changed={}", path.display());
    // Its name, not its path: the build machine's paths stay out of the
    // module.
    let origin = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;

    let contract = Contract::load(&text, &origin)
        .and_then(|loaded| loaded.into_enforceable(&origin))
        .map_err(|e| e.to_string())?;
    let compiled = CompiledContract::compile(&contract).map_err(|e| e.to_string())?;
    let model = std::env::var("COVENANT_MODEL").ok();
    let model = compiled
        .resolve_model(model.as_deref())
        .map_err(|e| e.to_string())?;
    let unique: Vec<&str> = model
        .fields
        .iter()
        .filter(|f| f.unique)
        .map(|f| f.name.as_str())
        .collect();
    let skip_unique = std::env::var("COVENANT_NO_UNIQUE").is_ok_and(|v| v == "1");
    if !unique.is_empty() && !skip_unique {
        return Err(format!(
            "model {} declares unique fields ({}), which a transform cannot hold across a topic: \
             it keeps its state per partition and loses it on restart. Set COVENANT_NO_UNIQUE=1 \
             to build without enforcing them",
            model.name,
            unique.join(", ")
        ));
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    write(&out.join("contract.yaml"), &text)?;
    write(&out.join("contract.origin"), &origin)?;
    write(&out.join("model"), &model.name)
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}
