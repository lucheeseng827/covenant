/*
[dependencies]
covenant = { package = "covenant-data", git = "https://github.com/lucheeseng827/covenant", default-features = false }
*/

// Covenant's verdict on one record, for Arroyo SQL: 'pass', 'block' or
// 'warn', judged by the same engine as `covenant gate`. Each call judges one
// record on its own, so `unique` is not enforced (render.py refuses a model
// that declares it unless told to skip it). A NULL record gives NULL.
//
// Rendered by render.py, which puts the contract below after checking it.

use std::sync::OnceLock;

use arroyo_udf_plugin::udf;
use covenant::prelude::{CompiledContract, CompiledModel, Contract, RecordGate, Verdict};

const CONTRACT: &str = r####"__COVENANT_CONTRACT__"####;
const ORIGIN: &str = "__COVENANT_ORIGIN__";
const MODEL: &str = "__COVENANT_MODEL__";
const SKIP_UNIQUE: bool = __COVENANT_SKIP_UNIQUE__;

fn model() -> (&'static CompiledContract, &'static CompiledModel) {
    static COMPILED: OnceLock<CompiledContract> = OnceLock::new();
    let contract = COMPILED.get_or_init(|| {
        Contract::load(CONTRACT, ORIGIN)
            .and_then(|loaded| loaded.into_enforceable(ORIGIN))
            .and_then(|contract| CompiledContract::compile(&contract))
            .expect("render.py checked this contract")
    });
    let model = contract
        .resolve_model((!MODEL.is_empty()).then_some(MODEL))
        .expect("render.py checked this model");
    // A call judges one record on its own, so it cannot hold `unique`:
    // refused, never quietly skipped, unless rendered with --no-unique.
    let unique: Vec<&str> = model.fields.iter().filter(|f| f.unique).map(|f| f.name.as_str()).collect();
    assert!(
        SKIP_UNIQUE || unique.is_empty(),
        "covenant: model {} declares unique fields ({}), which a UDF cannot hold; render with --no-unique to skip them",
        model.name,
        unique.join(", ")
    );
    (contract, model)
}

#[udf]
fn covenant_verdict(record: &str) -> String {
    let (contract, model) = model();
    let mut gate = RecordGate::new(contract, model, false);
    match gate.judge(record.as_bytes()) {
        Verdict::Pass => "pass",
        Verdict::Block => "block",
        Verdict::Warn => "warn",
    }
    .to_string()
}
