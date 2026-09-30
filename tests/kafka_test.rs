//! `covenant gate` over Kafka topics (the `kafka` feature), against a real
//! broker. The tests are ignored by default and run as
//!
//! ```text
//! COVENANT_TEST_KAFKA=localhost:9092 cargo test --features kafka --test kafka_test -- --ignored
//! ```
//!
//! against a broker that lets them create topics. CI's `kafka` job does
//! exactly that. Every test makes its own topics, so they can share one.
#![cfg(feature = "kafka")]

mod common;

use std::collections::HashMap;
use std::future::Future;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
use rdkafka::client::DefaultClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, Consumer};
use rdkafka::error::KafkaError;
use rdkafka::message::{Header, Headers, Message, OwnedHeaders};
use rdkafka::producer::{BaseProducer, BaseRecord, Producer};
use rdkafka::{Offset, TopicPartitionList};
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_covenant");
const NEEDS_BROKER: &str = "needs a Kafka broker: COVENANT_TEST_KAFKA=host:port";

fn brokers() -> String {
    std::env::var("COVENANT_TEST_KAFKA").expect(NEEDS_BROKER)
}

/// A name no other test run has used.
fn unique(name: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("covenant-test.{name}.{}.{nanos}", std::process::id())
}

/// Wait for a future on this thread: the admin client's replies are the only
/// futures here, and they complete on the client's own thread.
fn block_on<F: Future>(future: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
        std::thread::park_timeout(Duration::from_millis(100));
    }
}

/// Create a topic with `partitions` and the given topic configs.
fn create_topic(name: &str, partitions: i32, configs: &[(&str, &str)]) {
    let admin: AdminClient<DefaultClientContext> = ClientConfig::new()
        .set("bootstrap.servers", brokers())
        .create()
        .unwrap();
    let mut topic = NewTopic::new(name, partitions, TopicReplication::Fixed(1));
    for (key, value) in configs {
        topic = topic.set(key, value);
    }
    let options = AdminOptions::new().operation_timeout(Some(Duration::from_secs(30)));
    for result in block_on(admin.create_topics([&topic], &options)).unwrap() {
        result.unwrap_or_else(|(topic, e)| panic!("cannot create {topic}: {e}"));
    }
}

/// One record to produce: key, value (`None` = a tombstone), headers,
/// timestamp.
struct Rec {
    key: Option<&'static str>,
    value: Option<&'static str>,
    headers: &'static [(&'static str, &'static str)],
    timestamp: Option<i64>,
}

fn rec(key: &'static str, value: &'static str) -> Rec {
    Rec {
        key: Some(key),
        value: Some(value),
        headers: &[],
        timestamp: None,
    }
}

fn produce(topic: &str, records: &[Rec]) {
    let producer: BaseProducer = ClientConfig::new()
        .set("bootstrap.servers", brokers())
        .create()
        .unwrap();
    for r in records {
        let mut record = BaseRecord::<str, str>::to(topic);
        if let Some(key) = r.key {
            record = record.key(key);
        }
        if let Some(value) = r.value {
            record = record.payload(value);
        }
        if !r.headers.is_empty() {
            let mut headers = OwnedHeaders::new();
            for (key, value) in r.headers {
                headers = headers.insert(Header {
                    key,
                    value: Some(*value),
                });
            }
            record = record.headers(headers);
        }
        if let Some(ts) = r.timestamp {
            record = record.timestamp(ts);
        }
        producer.send(record).map_err(|(e, _)| e).unwrap();
    }
    producer.flush(Duration::from_secs(30)).unwrap();
}

/// A record as read back.
#[derive(Debug, Clone)]
struct Got {
    partition: i32,
    offset: i64,
    key: Option<String>,
    value: Option<Vec<u8>>,
    headers: Vec<(String, String)>,
    timestamp: Option<i64>,
}

impl Got {
    fn text(&self) -> &str {
        std::str::from_utf8(self.value.as_deref().unwrap()).unwrap()
    }
    fn json(&self) -> Value {
        serde_json::from_slice(self.value.as_deref().unwrap()).unwrap()
    }
    fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

fn partitions(topic: &str, consumer: &BaseConsumer) -> Vec<i32> {
    let metadata = consumer
        .fetch_metadata(Some(topic), Duration::from_secs(30))
        .unwrap();
    metadata.topics()[0]
        .partitions()
        .iter()
        .map(|p| p.id())
        .collect()
}

/// Every record in `topic`, partition by partition, in offset order.
fn read_all(topic: &str) -> Vec<Got> {
    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", brokers())
        .set("group.id", unique("reader"))
        .set("enable.partition.eof", "true")
        .create()
        .unwrap();
    let parts = partitions(topic, &consumer);
    let mut assignment = TopicPartitionList::new();
    for &p in &parts {
        assignment
            .add_partition_offset(topic, p, Offset::Beginning)
            .unwrap();
    }
    consumer.assign(&assignment).unwrap();
    let mut at_end = std::collections::HashSet::new();
    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while at_end.len() < parts.len() {
        assert!(Instant::now() < deadline, "reading {topic} timed out");
        match consumer.poll(Duration::from_millis(100)) {
            Some(Ok(m)) => got.push(Got {
                partition: m.partition(),
                offset: m.offset(),
                key: m.key().map(|k| String::from_utf8(k.to_vec()).unwrap()),
                value: m.payload().map(<[u8]>::to_vec),
                headers: m
                    .headers()
                    .map(|h| {
                        h.iter()
                            .map(|h| {
                                (
                                    h.key.to_string(),
                                    String::from_utf8(h.value.unwrap_or_default().to_vec())
                                        .unwrap(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                timestamp: m.timestamp().to_millis(),
            }),
            Some(Err(KafkaError::PartitionEOF(p))) => {
                at_end.insert(p);
            }
            Some(Err(e)) => panic!("reading {topic}: {e}"),
            None => {}
        }
    }
    got.sort_by_key(|g| (g.partition, g.offset));
    got
}

/// The group's committed offsets for `topic`, per partition (`None` where
/// nothing is committed).
fn committed(group: &str, topic: &str) -> HashMap<i32, Option<i64>> {
    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", brokers())
        .set("group.id", group)
        .create()
        .unwrap();
    let mut wanted = TopicPartitionList::new();
    for p in partitions(topic, &consumer) {
        wanted.add_partition(topic, p);
    }
    consumer
        .committed_offsets(wanted, Duration::from_secs(30))
        .unwrap()
        .elements()
        .iter()
        .map(|e| {
            let offset = match e.offset() {
                Offset::Offset(n) => Some(n),
                _ => None,
            };
            (e.partition(), offset)
        })
        .collect()
}

const CONTRACT: &str = "covenant: 1
id: payments
version: 1.0.0
policy:
  on_violation: POLICY
models:
  payments:
    fields:
      id: { type: string, required: true, unique: true }
      amount: { type: integer, required: true, min: 0 }
";

fn contract(dir: &Path, policy: &str) -> String {
    let path = dir.join(format!("payments-{policy}.yaml"));
    std::fs::write(&path, CONTRACT.replace("POLICY", policy)).unwrap();
    path.display().to_string()
}

/// Run the gate to the end of its input, killing it after a minute.
fn gate(args: &[&str]) -> Output {
    let child = Command::new(BIN)
        .arg("gate")
        .args(args)
        .args(["--brokers", &brokers(), "--exit-at-end"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait(child, Duration::from_secs(60))
}

fn wait(mut child: Child, limit: Duration) -> Output {
    let deadline = Instant::now() + limit;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            let out = child.wait_with_output().unwrap();
            panic!(
                "the gate did not stop: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    child.wait_with_output().unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The records every test sends: two that keep the contract, one keyless;
/// three that break it (a negative amount, not JSON, a repeated id); and a
/// tombstone.
fn payments() -> Vec<Rec> {
    vec![
        Rec {
            key: Some("k0"),
            value: Some(r#"{"id":"p0","amount":10}"#),
            headers: &[("trace", "t-0")],
            timestamp: Some(1_700_000_000_000),
        },
        rec("k1", r#"{"id":"p1","amount":-5}"#),
        rec("k2", "not json"),
        Rec {
            key: Some("k3"),
            value: None,
            headers: &[],
            timestamp: None,
        },
        rec("k4", r#"{"id":"p0","amount":1}"#),
        Rec {
            key: None,
            value: Some(r#"{"id":"p5","amount":5}"#),
            headers: &[],
            timestamp: None,
        },
    ]
}

#[test]
#[ignore = "needs a Kafka broker: COVENANT_TEST_KAFKA=host:port"]
fn the_gate_forwards_what_keeps_the_contract_and_dead_letters_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let (raw, clean, dlq) = (unique("raw"), unique("clean"), unique("dlq"));
    for topic in [&raw, &clean, &dlq] {
        create_topic(topic, 1, &[]);
    }
    produce(&raw, &payments());
    let group = unique("group");
    let contract = contract(dir.path(), "block");
    let args = [
        "-c",
        &contract,
        "--from",
        &raw,
        "--to",
        &clean,
        "--dlq-topic",
        &dlq,
        "--group",
        &group,
    ];

    let out = gate(&args);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out)
            .contains("5 records, 2 passed, 3 blocked, 0 warned, 1 tombstone passed on unjudged"),
        "{}",
        stderr(&out)
    );

    // What goes on is the record as it arrived: key, value, headers and
    // timestamp, in order, the tombstone included.
    let forwarded = read_all(&clean);
    let keys: Vec<_> = forwarded.iter().map(|g| g.key.as_deref()).collect();
    assert_eq!(keys, [Some("k0"), Some("k3"), None]);
    assert_eq!(forwarded[0].text(), r#"{"id":"p0","amount":10}"#);
    assert_eq!(forwarded[0].header("trace"), Some("t-0"));
    assert_eq!(forwarded[0].timestamp, Some(1_700_000_000_000));
    assert!(
        forwarded[1].value.is_none(),
        "the tombstone stays a tombstone"
    );
    assert_eq!(forwarded[2].text(), r#"{"id":"p5","amount":5}"#);

    // Each dead letter keeps the record's key and says where it came from.
    let letters = read_all(&dlq);
    let keys: Vec<_> = letters.iter().map(|g| g.key.as_deref()).collect();
    assert_eq!(keys, [Some("k1"), Some("k2"), Some("k4")]);
    let rules: Vec<_> = letters
        .iter()
        .map(|g| {
            g.json()["violations"][0]["rule"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(rules, ["min", "record_not_object", "unique"]);
    let first = &letters[0];
    assert_eq!(first.json()["contract_id"], "payments");
    assert_eq!(first.json()["record"]["amount"], -5);
    assert_eq!(first.header("covenant.source.topic"), Some(raw.as_str()));
    assert_eq!(first.header("covenant.source.partition"), Some("0"));
    assert_eq!(first.header("covenant.source.offset"), Some("1"));
    assert_eq!(letters[1].json()["record"], "not json");

    // Everything is committed, so the same group has nothing left to do.
    assert_eq!(committed(&group, &raw), HashMap::from([(0, Some(6))]));
    let again = gate(&args);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    assert!(stderr(&again).contains(": 0 records"), "{}", stderr(&again));
    assert_eq!(read_all(&clean).len(), 3, "nothing is forwarded twice");
}

#[test]
#[ignore = "needs a Kafka broker: COVENANT_TEST_KAFKA=host:port"]
fn the_gate_reports_its_topic_and_what_became_of_the_records() {
    let dir = tempfile::tempdir().unwrap();
    let (raw, clean) = (unique("raw"), unique("clean"));
    create_topic(&raw, 1, &[]);
    create_topic(&clean, 1, &[]);
    produce(&raw, &payments());
    let letters = dir.path().join("dlq.ndjson");
    let report = dir.path().join("report.json");
    let contract = contract(dir.path(), "block");
    let (lineage, events) = common::server(200);

    let out = gate(&[
        "-c",
        &contract,
        "--from",
        &raw,
        "--to",
        &clean,
        "--dlq",
        letters.to_str().unwrap(),
        "--report-json",
        report.to_str().unwrap(),
        "--openlineage",
        &lineage,
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("report: covenant-report/v1 → "),
        "{}",
        stderr(&out)
    );
    let raw_doc = std::fs::read_to_string(&report).unwrap();
    let doc: Value = serde_json::from_str(&raw_doc).unwrap();
    assert_eq!(doc["kind"], "gate");
    assert_eq!(doc["verdict"], "fail");
    assert_eq!(doc["run"]["plane"], "gate");
    let first = brokers().split(',').next().unwrap().trim().to_string();
    assert_eq!(doc["checks"][0]["source"], format!("kafka://{first}/{raw}"));
    assert_eq!(doc["checks"][0]["rows"], 5);
    assert_eq!(doc["violations"], 3);
    assert_eq!(
        doc["gate"],
        serde_json::json!({ "passed": 2, "blocked": 3, "warned": 0, "tombstones": 1 })
    );
    // A sample's row is its dead letter's; the value stays in the dead letter.
    let rows: Vec<u64> = std::fs::read_to_string(&letters)
        .unwrap()
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["row"]
                .as_u64()
                .unwrap()
        })
        .collect();
    for sample in doc["checks"][0]["samples"].as_array().unwrap() {
        assert!(rows.contains(&sample["row"].as_u64().unwrap()), "{sample}");
    }
    assert!(!raw_doc.contains("not json"), "a value leaked: {raw_doc}");
    let schema: Value =
        serde_json::from_str(include_str!("../schema/covenant-report.v1.json")).unwrap();
    let errors: Vec<String> = jsonschema::validator_for(&schema)
        .unwrap()
        .iter_errors(&doc)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    // OpenLineage names the topic as it names a Kafka dataset.
    assert_eq!(events.recv().unwrap().json()["eventType"], "START");
    let complete = events.recv().unwrap().json();
    let input = &complete["inputs"][0];
    assert_eq!(input["namespace"], format!("kafka://{first}"));
    assert_eq!(input["name"], raw.as_str());
    assert_eq!(complete["job"]["name"], "gate.payments.payments");
    assert_eq!(input["inputFacets"]["dataQualityMetrics"]["rowCount"], 5);
}

#[test]
#[ignore = "needs a Kafka broker: COVENANT_TEST_KAFKA=host:port"]
fn under_warn_everything_goes_on_and_dead_letters_go_to_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let (raw, clean) = (unique("raw"), unique("clean"));
    create_topic(&raw, 1, &[]);
    create_topic(&clean, 1, &[]);
    produce(&raw, &payments());
    let letters = dir.path().join("dlq.ndjson");
    let contract = contract(dir.path(), "warn");

    let out = gate(&[
        "-c",
        &contract,
        "--from",
        &raw,
        "--to",
        &clean,
        "--dlq",
        letters.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("5 records, 5 passed, 0 blocked, 3 warned"),
        "{}",
        stderr(&out)
    );
    assert_eq!(read_all(&clean).len(), 6, "five records and the tombstone");
    let lines = std::fs::read_to_string(&letters).unwrap();
    assert_eq!(lines.lines().count(), 3, "{lines}");
    // The default group: one per contract and source topic.
    let group = format!("covenant-gate.payments.{raw}");
    assert_eq!(committed(&group, &raw), HashMap::from([(0, Some(6))]));
}

#[test]
#[ignore = "needs a Kafka broker: COVENANT_TEST_KAFKA=host:port"]
fn a_record_the_brokers_refuse_stops_the_gate_and_commits_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (raw, clean) = (unique("raw"), unique("clean"));
    // A DLQ that takes nothing as large as a dead letter.
    let (small, roomy) = (unique("dlq-small"), unique("dlq"));
    create_topic(&raw, 1, &[]);
    create_topic(&clean, 1, &[]);
    create_topic(&small, 1, &[("max.message.bytes", "200")]);
    create_topic(&roomy, 1, &[]);
    produce(&raw, &payments());
    let group = unique("group");
    let contract = contract(dir.path(), "block");

    let out = gate(&[
        "-c",
        &contract,
        "--from",
        &raw,
        "--to",
        &clean,
        "--dlq-topic",
        &small,
        "--group",
        &group,
    ]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("was not delivered"),
        "{}",
        stderr(&out)
    );
    assert_eq!(committed(&group, &raw), HashMap::from([(0, None)]));

    // The same group, a DLQ that takes them: every record is judged again
    // and nothing is lost (what went on the first time goes on again).
    let out = gate(&[
        "-c",
        &contract,
        "--from",
        &raw,
        "--to",
        &clean,
        "--dlq-topic",
        &roomy,
        "--group",
        &group,
    ]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(read_all(&roomy).len(), 3);
    let forwarded = read_all(&clean);
    for key in [Some("k0"), Some("k3"), None] {
        assert!(
            forwarded.iter().any(|g| g.key.as_deref() == key),
            "{key:?} forwarded at least once"
        );
    }
    assert_eq!(committed(&group, &raw), HashMap::from([(0, Some(6))]));
}

#[test]
#[ignore = "needs a Kafka broker: COVENANT_TEST_KAFKA=host:port"]
fn the_gate_refuses_a_topic_that_does_not_exist() {
    let dir = tempfile::tempdir().unwrap();
    let raw = unique("raw");
    create_topic(&raw, 1, &[]);
    let missing = unique("missing");
    let contract = contract(dir.path(), "block");

    let out = gate(&["-c", &contract, "--from", &raw, "--to", &missing]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(&format!("topic {missing} does not exist")),
        "{}",
        stderr(&out)
    );
}

/// Kafka's own murmur2, as the Java client's default partitioner applies it
/// to a key.
fn java_partition(key: &[u8], partitions: u32) -> i32 {
    const M: u32 = 0x5bd1_e995;
    let mut h: u32 = 0x9747_b28c ^ key.len() as u32;
    let mut chunks = key.chunks_exact(4);
    for c in &mut chunks {
        let mut k = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        k = k.wrapping_mul(M);
        k ^= k >> 24;
        k = k.wrapping_mul(M);
        h = h.wrapping_mul(M) ^ k;
    }
    let rest = chunks.remainder();
    if rest.len() == 3 {
        h ^= u32::from(rest[2]) << 16;
    }
    if rest.len() >= 2 {
        h ^= u32::from(rest[1]) << 8;
    }
    if !rest.is_empty() {
        h ^= u32::from(rest[0]);
        h = h.wrapping_mul(M);
    }
    h ^= h >> 13;
    h = h.wrapping_mul(M);
    h ^= h >> 15;
    ((h & 0x7fff_ffff) % partitions) as i32
}

#[test]
fn the_java_partitioner_as_kafka_computes_it() {
    // Values from org.apache.kafka.common.utils.Utils.murmur2.
    assert_eq!(
        java_partition(b"21", 1 << 31),
        -973_932_308i32 & 0x7fff_ffff
    );
    assert_eq!(
        java_partition(b"foobar", 1 << 31),
        -790_332_482i32 & 0x7fff_ffff
    );
    assert_eq!(
        java_partition(b"a-little-bit-long-string", 1 << 31),
        -985_981_536i32 & 0x7fff_ffff
    );
    assert_eq!(
        java_partition(b"a-little-bit-longer-string", 1 << 31),
        -1_486_304_829i32 & 0x7fff_ffff
    );
    assert_eq!(
        java_partition(b"lkjh234lh9fiuh90y23oiuhsafujhadof229phr9h19h89h8", 1 << 31),
        -58_897_971i32 & 0x7fff_ffff
    );
}

#[test]
#[ignore = "needs a Kafka broker: COVENANT_TEST_KAFKA=host:port"]
fn a_key_lands_where_a_java_producer_would_put_it() {
    let dir = tempfile::tempdir().unwrap();
    let (raw, clean) = (unique("raw"), unique("clean"));
    create_topic(&raw, 1, &[]);
    create_topic(&clean, 4, &[]);
    let keys: Vec<&'static str> = (0..40)
        .map(|i| &*Box::leak(format!("customer-{i}").into_boxed_str()))
        .collect();
    let values: Vec<&'static str> = (0..40)
        .map(|i| &*Box::leak(format!(r#"{{"id":"p{i}","amount":{i}}}"#).into_boxed_str()))
        .collect();
    let records: Vec<Rec> = keys.iter().zip(&values).map(|(k, v)| rec(k, v)).collect();
    produce(&raw, &records);
    let contract = contract(dir.path(), "block");

    let out = gate(&["-c", &contract, "--from", &raw, "--to", &clean, "-q"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stderr(&out).is_empty(),
        "-q prints nothing: {}",
        stderr(&out)
    );
    let forwarded = read_all(&clean);
    assert_eq!(forwarded.len(), 40);
    let spread: std::collections::HashSet<_> = forwarded.iter().map(|g| g.partition).collect();
    assert!(spread.len() > 1, "40 keys over 4 partitions: {spread:?}");
    for g in &forwarded {
        let key = g.key.as_deref().unwrap();
        assert_eq!(g.partition, java_partition(key.as_bytes(), 4), "{key}");
    }
}

#[test]
#[ignore = "needs a Kafka broker: COVENANT_TEST_KAFKA=host:port"]
fn a_signal_settles_and_commits_before_the_gate_stops() {
    let dir = tempfile::tempdir().unwrap();
    let (raw, clean, dlq) = (unique("raw"), unique("clean"), unique("dlq"));
    for topic in [&raw, &clean, &dlq] {
        create_topic(topic, 1, &[]);
    }
    produce(&raw, &payments());
    let group = unique("group");
    let contract = contract(dir.path(), "block");

    // No --exit-at-end: it runs until it is told to stop.
    let mut child = Command::new(BIN)
        .args(["gate", "-c", &contract, "--brokers", &brokers()])
        .args([
            "--from",
            &raw,
            "--to",
            &clean,
            "--dlq-topic",
            &dlq,
            "--group",
            &group,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while read_all(&dlq).len() < 3 {
        assert!(Instant::now() < deadline, "the gate never caught up");
        std::thread::sleep(Duration::from_millis(250));
    }
    let status = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let out = wait_child(&mut child);
    assert_eq!(out.0, Some(1), "{}", out.1);
    assert!(
        out.1.contains("5 records, 2 passed, 3 blocked"),
        "{}",
        out.1
    );
    assert_eq!(committed(&group, &raw), HashMap::from([(0, Some(6))]));
}

fn wait_child(child: &mut Child) -> (Option<i32>, String) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the gate did not stop on SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    (status.code(), err)
}

/// `count` keyed records from `from` on: every tenth breaks the contract.
fn numbered(from: usize, count: usize) -> Vec<Rec> {
    (from..from + count)
        .map(|i| {
            let amount = if i % 10 == 0 { -1 } else { i as i64 };
            let key = Box::leak(format!("key-{i}").into_boxed_str());
            let value = Box::leak(format!(r#"{{"id":"p{i}","amount":{amount}}}"#).into_boxed_str());
            rec(key, value)
        })
        .collect()
}

/// Every record id in `clean` and in `dlq`, with how often it appears.
fn delivered(clean: &str, dlq: &str) -> HashMap<String, usize> {
    let mut ids = HashMap::new();
    for g in read_all(clean) {
        *ids.entry(g.json()["id"].as_str().unwrap().to_string())
            .or_default() += 1;
    }
    for g in read_all(dlq) {
        *ids.entry(g.json()["record"]["id"].as_str().unwrap().to_string())
            .or_default() += 1;
    }
    ids
}

fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(90);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
#[ignore = "needs a Kafka broker: COVENANT_TEST_KAFKA=host:port"]
fn a_rebalance_settles_so_the_next_owner_starts_where_the_gate_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let (raw, clean, dlq) = (unique("raw"), unique("clean"), unique("dlq"));
    create_topic(&raw, 4, &[]);
    create_topic(&clean, 4, &[]);
    create_topic(&dlq, 1, &[]);
    let group = unique("group");
    let contract = contract(dir.path(), "block");
    // The periodic settle is an hour away, so only a rebalance (or a stop)
    // commits what the gates have done.
    let start = || {
        Command::new(BIN)
            .args(["gate", "-c", &contract, "--brokers", &brokers()])
            .args(["--from", &raw, "--to", &clean, "--dlq-topic", &dlq])
            .args(["--group", &group, "--commit-interval-ms", "3600000"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };

    produce(&raw, &numbered(0, 200));
    let mut first = start();
    eventually("the first gate reads 200 records", || {
        delivered(&clean, &dlq).len() == 200
    });
    assert_eq!(
        committed(&group, &raw).values().flatten().count(),
        0,
        "nothing committed yet"
    );

    // A second gate joins: the first is revoked, and must settle first.
    let mut second = start();
    let admin: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", brokers())
        .create()
        .unwrap();
    eventually("both gates hold partitions", || {
        admin
            .fetch_group_list(Some(&group), Duration::from_secs(10))
            .is_ok_and(|list| {
                list.groups()
                    .first()
                    .is_some_and(|g| g.state() == "Stable" && g.members().len() == 2)
            })
    });
    produce(&raw, &numbered(200, 200));
    eventually("the two gates read 400 records", || {
        delivered(&clean, &dlq).len() == 400
    });

    for child in [&mut first, &mut second] {
        let status = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
    }
    let (a, b) = (wait_child(&mut first), wait_child(&mut second));
    assert_eq!(a.0, Some(1), "{}", a.1);
    assert!(matches!(b.0, Some(0 | 1)), "{}", b.1);

    // Exactly once each: had the first gate not committed when it was
    // revoked, the second would have read its partitions from the start.
    let ids = delivered(&clean, &dlq);
    assert_eq!(ids.len(), 400);
    let twice: Vec<_> = ids.iter().filter(|(_, &n)| n > 1).collect();
    assert!(twice.is_empty(), "records produced twice: {twice:?}");
    let offsets = committed(&group, &raw);
    assert_eq!(offsets.values().map(|o| o.unwrap_or(0)).sum::<i64>(), 400);
}
