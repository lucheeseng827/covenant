//! Covenant as a Redpanda Data Transform: the contract embedded at build
//! time (see `build.rs`), every record written to the input topic judged by
//! the same `RecordGate` as `covenant gate`, the records that go on written
//! to the first output topic and dead letters to `COVENANT_DLQ_TOPIC`, which
//! must be one of the transform's output topics.

use std::cell::RefCell;

use covenant::prelude::{CompiledContract, Contract, RecordGate};
use covenant_redpanda_transform::gate_record;
use redpanda_transform_sdk::on_record_written;

const CONTRACT: &str = include_str!(concat!(env!("OUT_DIR"), "/contract.yaml"));
const ORIGIN: &str = include_str!(concat!(env!("OUT_DIR"), "/contract.origin"));
const MODEL: &str = include_str!(concat!(env!("OUT_DIR"), "/model"));

fn main() {
    // A dead letter needs somewhere to go: a transform without one would
    // drop what it withholds, so it does not start.
    let dlq = std::env::var("COVENANT_DLQ_TOPIC").unwrap_or_else(|_| {
        panic!("set COVENANT_DLQ_TOPIC (rpk transform deploy --var COVENANT_DLQ_TOPIC=<topic>) to one of the output topics")
    });
    // The build compiled this very contract; it cannot fail here.
    let contract = Contract::load(CONTRACT, ORIGIN)
        .and_then(|loaded| loaded.into_enforceable(ORIGIN))
        .and_then(|contract| CompiledContract::compile(&contract))
        .expect("the contract compiled at build time");
    let contract: &'static CompiledContract = Box::leak(Box::new(contract));
    let model = contract
        .resolve_model(Some(MODEL))
        .expect("the model resolved at build time");
    // State lives per partition and ends with the instance, so `unique` is
    // not tracked; the build refuses a model that declares it unless told to.
    let gate = RefCell::new(RecordGate::new(contract, model, false));
    on_record_written(move |event, writer| {
        gate_record(&mut gate.borrow_mut(), &dlq, &event, writer).map(|_| ())
    })
}
