//! The gate for WebAssembly hosts: `RecordGate` behind a C ABI, built as a
//! reactor module (`cargo build --release --target wasm32-wasip1`).
//!
//! One instance holds one gate. A host instantiates the module, calls
//! `_initialize` (the WASI reactor start), opens the gate once and then judges
//! records one at a time; to gate another contract it instantiates the module
//! again. Everything the gate keeps lives in the instance's memory and goes
//! with it.
//!
//! Bytes cross in both directions through the instance's memory. The host
//! asks for a buffer with `covenant_alloc`, writes into it, passes pointer and
//! length, and gives it back with `covenant_free`. A result the module returns
//! is packed into an `i64`, pointer in the high 32 bits and length in the low
//! 32; it stays valid until the next call into the module.
//!
//! | Export | Returns |
//! |---|---|
//! | `covenant_alloc(len) -> ptr` | a buffer of `len` bytes |
//! | `covenant_free(ptr, len)` | |
//! | `covenant_open(contract, contract_len, origin, origin_len, model, model_len, unique) -> i32` | 0, or 1 with the reason in `covenant_error` |
//! | `covenant_judge(record, record_len) -> i32` | 0 pass, 1 block, 2 warn; -1 when no gate is open |
//! | `covenant_dead_letter() -> i64` | the dead letter of the record just judged (JSON), empty after a pass |
//! | `covenant_stats() -> i64` | `{"records", "passed", "blocked", "warned", "failed"}` (JSON) |
//! | `covenant_error() -> i64` | why the last `covenant_open` failed (UTF-8) |
//!
//! `unique` says what to do with a model's `unique` fields: 0 refuses a model
//! that declares any (the open fails and names them), 1 skips them, 2 tracks
//! them, exactly, across what this instance judges.

use std::cell::RefCell;

use covenant::prelude::{CompiledContract, Contract, RecordGate, Verdict};

/// The gate this instance holds, and the bytes of its last answer.
struct State {
    gate: Option<RecordGate<'static>>,
    out: Vec<u8>,
}

thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State { gate: None, out: Vec::new() }) };
}

/// Pack a pointer and a length into one `i64` for the host.
fn packed(bytes: &[u8]) -> i64 {
    ((bytes.as_ptr() as u32 as i64) << 32) | bytes.len() as u32 as i64
}

/// Answer with `bytes`, kept until the next call.
fn answer(state: &mut State, bytes: Vec<u8>) -> i64 {
    state.out = bytes;
    packed(&state.out)
}

/// # Safety
///
/// The host must pass pointers this module handed out, with their lengths.
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(ptr, len)
    }
}

/// A buffer of `len` bytes for the host to write into.
#[no_mangle]
pub extern "C" fn covenant_alloc(len: usize) -> *mut u8 {
    let mut buffer = Vec::<u8>::with_capacity(len.max(1));
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// Give back a buffer from [`covenant_alloc`].
///
/// # Safety
///
/// `ptr` and `len` must be a buffer [`covenant_alloc`] returned, given back once.
#[no_mangle]
pub unsafe extern "C" fn covenant_free(ptr: *mut u8, len: usize) {
    drop(Vec::from_raw_parts(ptr, 0, len.max(1)));
}

/// Open the gate: load, lint and compile the contract, exactly as
/// `covenant gate` does, and refuse one this runtime cannot enforce.
///
/// # Safety
///
/// Every pointer and length must describe bytes in this instance's memory.
#[no_mangle]
pub unsafe extern "C" fn covenant_open(
    contract: *const u8,
    contract_len: usize,
    origin: *const u8,
    origin_len: usize,
    model: *const u8,
    model_len: usize,
    unique: i32,
) -> i32 {
    let text = String::from_utf8_lossy(bytes(contract, contract_len)).into_owned();
    let origin = String::from_utf8_lossy(bytes(origin, origin_len)).into_owned();
    let model = String::from_utf8_lossy(bytes(model, model_len)).into_owned();
    STATE.with_borrow_mut(|state| {
        let opened = if state.gate.is_some() {
            Err("this instance already holds a gate; instantiate the module again".to_string())
        } else {
            open(&text, &origin, &model, unique)
        };
        match opened {
            Ok(gate) => {
                state.gate = Some(gate);
                0
            }
            Err(message) => {
                state.out = message.into_bytes();
                1
            }
        }
    })
}

fn open(text: &str, origin: &str, model: &str, unique: i32) -> Result<RecordGate<'static>, String> {
    let contract = Contract::load(text, origin)
        .and_then(|loaded| loaded.into_enforceable(origin))
        .and_then(|contract| CompiledContract::compile(&contract))
        .map_err(|e| e.to_string())?;
    // It lives as long as the instance does.
    let contract: &'static CompiledContract = Box::leak(Box::new(contract));
    let model = contract
        .resolve_model((!model.is_empty()).then_some(model))
        .map_err(|e| e.to_string())?;
    let declared: Vec<&str> = model
        .fields
        .iter()
        .filter(|f| f.unique)
        .map(|f| f.name.as_str())
        .collect();
    let track = match unique {
        0 if !declared.is_empty() => {
            return Err(format!(
                "model {} declares unique fields ({}); open it to skip them or to track them",
                model.name,
                declared.join(", ")
            ))
        }
        0 | 1 => false,
        2 => true,
        other => return Err(format!("unique must be 0, 1 or 2, not {other}")),
    };
    Ok(RecordGate::new(contract, model, track))
}

/// Judge one record: 0 pass, 1 block, 2 warn; -1 when no gate is open.
///
/// # Safety
///
/// `record` and `len` must describe bytes in this instance's memory.
#[no_mangle]
pub unsafe extern "C" fn covenant_judge(record: *const u8, len: usize) -> i32 {
    let record = bytes(record, len);
    STATE.with_borrow_mut(|state| match state.gate.as_mut() {
        None => -1,
        Some(gate) => match gate.judge(record) {
            Verdict::Pass => 0,
            Verdict::Block => 1,
            Verdict::Warn => 2,
        },
    })
}

/// The dead letter of the record just judged, as `covenant gate` writes it;
/// empty when it passed.
#[no_mangle]
pub extern "C" fn covenant_dead_letter() -> i64 {
    STATE.with_borrow_mut(|state| {
        let letter = match state.gate.as_ref() {
            Some(gate) if !gate.violations().is_empty() => {
                serde_json::to_vec(&gate.dead_letter()).expect("a dead letter serializes")
            }
            _ => Vec::new(),
        };
        answer(state, letter)
    })
}

/// The counts so far, and whether enforcement fails the stream.
#[no_mangle]
pub extern "C" fn covenant_stats() -> i64 {
    STATE.with_borrow_mut(|state| {
        let stats = match state.gate.as_ref() {
            Some(gate) => {
                let s = gate.stats();
                serde_json::json!({
                    "records": s.records,
                    "passed": s.passed,
                    "blocked": s.blocked,
                    "warned": s.warned,
                    "failed": gate.failed(),
                })
            }
            None => serde_json::Value::Null,
        };
        answer(state, stats.to_string().into_bytes())
    })
}

/// Why the last [`covenant_open`] failed.
#[no_mangle]
pub extern "C" fn covenant_error() -> i64 {
    STATE.with_borrow(|state| packed(&state.out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_with(yaml: &str, unique: i32) -> Result<(), String> {
        let origin = "t.yaml";
        let rc = unsafe {
            covenant_open(
                yaml.as_ptr(),
                yaml.len(),
                origin.as_ptr(),
                origin.len(),
                [].as_ptr(),
                0,
                unique,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(STATE.with_borrow(|s| String::from_utf8(s.out.clone()).unwrap()))
        }
    }

    fn unpack(r: i64) -> Vec<u8> {
        let (ptr, len) = ((r >> 32) as u32 as usize, r as u32 as usize);
        // On a 64-bit host the pointer does not fit in 32 bits, so the tests
        // read the answer from the state instead.
        let _ = ptr;
        STATE.with_borrow(|s| s.out[..len].to_vec())
    }

    const YAML: &str = "covenant: 1\nid: t\nversion: 1.0.0\nmodels:\n  t:\n    fields:\n      id: { type: string, unique: true }\n      n: { type: integer, min: 0 }\n";

    #[test]
    fn the_abi_judges_and_explains() {
        // Each test thread is its own "instance".
        assert!(open_with(YAML, 0)
            .unwrap_err()
            .contains("declares unique fields (id)"));
        open_with(YAML, 2).unwrap();
        assert!(open_with(YAML, 2)
            .unwrap_err()
            .contains("already holds a gate"));

        let ok = br#"{"id":"a","n":1}"#;
        assert_eq!(unsafe { covenant_judge(ok.as_ptr(), ok.len()) }, 0);
        assert!(unpack(covenant_dead_letter()).is_empty());

        let again = br#"{"id":"a","n":-1}"#;
        assert_eq!(unsafe { covenant_judge(again.as_ptr(), again.len()) }, 1);
        let letter: serde_json::Value =
            serde_json::from_slice(&unpack(covenant_dead_letter())).unwrap();
        let rules: Vec<_> = letter["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["rule"].as_str().unwrap())
            .collect();
        assert_eq!(rules, ["unique", "min"]);

        let stats: serde_json::Value = serde_json::from_slice(&unpack(covenant_stats())).unwrap();
        assert_eq!(
            stats,
            serde_json::json!({"records": 2, "passed": 1, "blocked": 1, "warned": 0, "failed": true})
        );
    }

    #[test]
    fn judging_without_a_gate_says_so() {
        assert_eq!(unsafe { covenant_judge([].as_ptr(), 0) }, -1);
    }

    #[test]
    fn a_broken_contract_is_refused_with_its_reason() {
        let err = open_with("covenant: 1\nid: t\nversion: nope\nmodels: {}\n", 1).unwrap_err();
        assert!(err.contains("not valid semver"), "{err}");
    }
}
