//! The transform's judgement of one record, apart from the broker glue in
//! `main.rs` so that it runs in tests on any target.

use covenant::prelude::{RecordGate, Verdict};
use redpanda_transform_sdk::{BorrowedRecord, RecordWriter, WriteError, WriteEvent, WriteOptions};

/// Judge the record `event` carries and write what follows from the verdict:
/// the record, unchanged, to the transform's first output topic if it goes
/// on, and its dead letter to `dlq` if it breaks the contract. A record with
/// no value, a tombstone, is not a record the contract describes: it goes on
/// unjudged, and the verdict is `None`.
///
/// A dead letter keeps the record's key and headers; its value is the
/// envelope `covenant gate` writes.
pub fn gate_record(
    gate: &mut RecordGate<'_>,
    dlq: &str,
    event: &WriteEvent<'_>,
    writer: &mut RecordWriter<'_>,
) -> Result<Option<Verdict>, WriteError> {
    let record = &event.record;
    let Some(value) = record.value() else {
        writer.write(record)?;
        return Ok(None);
    };
    let verdict = gate.judge(value);
    if verdict != Verdict::Pass {
        // An envelope is plain data: serializing it cannot fail.
        let letter = serde_json::to_vec(&gate.dead_letter()).expect("a dead letter serializes");
        writer.write_with_options(
            BorrowedRecord::new_with_headers(
                record.key(),
                Some(&letter),
                record.headers().to_vec(),
            ),
            WriteOptions::to_topic(dlq),
        )?;
    }
    if verdict != Verdict::Block {
        writer.write(record)?;
    }
    Ok(Some(verdict))
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use covenant::prelude::{CompiledContract, Contract};
    use redpanda_transform_sdk::{BorrowedHeader, RecordSink, WrittenRecord};

    use super::*;

    /// What a transform wrote: the topic (`None` = the first output topic),
    /// key, value and headers.
    type Written = (
        Option<String>,
        Option<Vec<u8>>,
        Option<Vec<u8>>,
        Vec<(Vec<u8>, Vec<u8>)>,
    );

    #[derive(Default)]
    struct Sink(Vec<Written>);

    impl RecordSink for Sink {
        fn write(
            &mut self,
            r: BorrowedRecord<'_>,
            opts: WriteOptions<'_>,
        ) -> Result<(), WriteError> {
            self.0.push((
                opts.topic.map(str::to_string),
                r.key().map(<[u8]>::to_vec),
                r.value().map(<[u8]>::to_vec),
                r.headers()
                    .iter()
                    .map(|h| (h.key().to_vec(), h.value().unwrap_or_default().to_vec()))
                    .collect(),
            ));
            Ok(())
        }
    }

    fn contract(policy: &str) -> CompiledContract {
        let yaml = format!(
            "covenant: 1\nid: payments\nversion: 1.0.0\npolicy: {{ on_violation: {policy} }}\n\
             models: {{ payments: {{ fields: {{ amount: {{ type: integer, required: true, min: 0 }} }} }} }}\n"
        );
        CompiledContract::compile(&Contract::parse(&yaml, "payments.yaml").unwrap()).unwrap()
    }

    /// A record's key and value.
    type KeyValue<'a> = (Option<&'a [u8]>, Option<&'a [u8]>);

    /// Run `records` through a gate under `policy`; what was written, and the verdicts.
    fn run(policy: &str, records: &[KeyValue<'_>]) -> (Vec<Written>, Vec<Option<Verdict>>) {
        let contract = contract(policy);
        let model = contract.resolve_model(None).unwrap();
        let mut gate = RecordGate::new(&contract, model, false);
        let mut sink = Sink::default();
        let mut verdicts = Vec::new();
        let trace = [BorrowedHeader::new(b"trace", Some(b"t-1"))];
        for (key, value) in records {
            let event = WriteEvent {
                record: WrittenRecord::new_with_headers(
                    *key,
                    *value,
                    SystemTime::UNIX_EPOCH,
                    trace.to_vec(),
                ),
            };
            let mut writer = RecordWriter::new(&mut sink);
            verdicts.push(gate_record(&mut gate, "payments_dlq", &event, &mut writer).unwrap());
        }
        (sink.0, verdicts)
    }

    #[test]
    fn a_clean_record_goes_on_unchanged() {
        let (written, verdicts) = run("block", &[(Some(b"k"), Some(br#"{"amount":1}"#))]);
        assert_eq!(verdicts, [Some(Verdict::Pass)]);
        assert_eq!(
            written,
            [(
                None,
                Some(b"k".to_vec()),
                Some(br#"{"amount":1}"#.to_vec()),
                vec![(b"trace".to_vec(), b"t-1".to_vec())]
            )]
        );
    }

    #[test]
    fn a_blocked_record_becomes_a_dead_letter_with_its_key_and_headers() {
        let (written, verdicts) = run("block", &[(Some(b"k"), Some(br#"{"amount":-1}"#))]);
        assert_eq!(verdicts, [Some(Verdict::Block)]);
        assert_eq!(written.len(), 1, "withheld from the output topic");
        let (topic, key, value, headers) = &written[0];
        assert_eq!(topic.as_deref(), Some("payments_dlq"));
        assert_eq!(key.as_deref(), Some(&b"k"[..]));
        assert_eq!(headers, &[(b"trace".to_vec(), b"t-1".to_vec())]);
        let letter: serde_json::Value = serde_json::from_slice(value.as_deref().unwrap()).unwrap();
        assert_eq!(letter["contract_id"], "payments");
        assert_eq!(letter["record"]["amount"], -1);
        assert_eq!(letter["violations"][0]["rule"], "min");
    }

    #[test]
    fn under_warn_the_record_goes_on_and_is_dead_lettered() {
        let (written, verdicts) = run("warn", &[(None, Some(b"not json"))]);
        assert_eq!(verdicts, [Some(Verdict::Warn)]);
        let topics: Vec<_> = written.iter().map(|w| w.0.as_deref()).collect();
        assert_eq!(topics, [Some("payments_dlq"), None]);
        assert_eq!(written[1].2.as_deref(), Some(&b"not json"[..]));
    }

    #[test]
    fn a_tombstone_goes_on_unjudged() {
        let (written, verdicts) = run("block", &[(Some(b"k"), None)]);
        assert_eq!(verdicts, [None]);
        assert_eq!(written.len(), 1);
        assert_eq!(
            (written[0].0.as_deref(), written[0].2.as_deref()),
            (None, None)
        );
    }
}
