//! The stream gate over Kafka topics (the `kafka` feature): consume a topic,
//! judge every record with [`RecordGate`] — the judge the line gate and
//! `--subprocess` use — and produce the records that go on to a second topic,
//! with their key, headers and timestamp unchanged, and dead letters to a
//! third topic or to a file.
//!
//! Delivery is at-least-once. An offset is committed only when every record
//! produced for what came before it has been acknowledged by the brokers and
//! every dead letter written to a file has been flushed: every second (or
//! `--commit-interval-ms`), before partitions are revoked in a rebalance, and
//! at the end. A delivery that
//! fails stops the gate (exit 2) without committing, so after a crash, a
//! failed delivery or a lost commit, the records since the last commit are
//! judged and produced again, never lost. Within one run a record read twice
//! (a partition handed back after a commit failed) is not judged twice, so
//! its own `unique` keys cannot convict it.
//!
//! A record with no value — a tombstone, the delete marker of a compacted
//! topic — is not a record the contract describes: it goes on unjudged, so a
//! compacted topic downstream still sees the delete.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rdkafka::client::ClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, CommitMode, Consumer, ConsumerContext, Rebalance};
use rdkafka::error::{KafkaError, RDKafkaErrorCode};
use rdkafka::message::{BorrowedMessage, Header, Message, OwnedHeaders};
use rdkafka::producer::{BaseProducer, BaseRecord, DeliveryResult, Producer, ProducerContext};
use rdkafka::{Offset, TopicPartitionList};

use crate::compile::{CompiledContract, CompiledModel};
use crate::error::{CovenantError, Result};
use crate::gate::{count_into, GateStats, RecordGate, StatsSink, Verdict};
use crate::report::Collector;

/// How long one settle may wait for the brokers to acknowledge.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the brokers get to answer before the first record.
const METADATA_TIMEOUT: Duration = Duration::from_secs(30);
/// One consumer poll; also how quickly a stop request is noticed.
const POLL: Duration = Duration::from_millis(100);
/// A recoverable client error is repeated on stderr at most this often.
const WARN_EVERY: Duration = Duration::from_secs(30);

/// Properties the gate sets itself, because at-least-once depends on them.
const OWN_PROPERTIES: &[(&str, &str)] = &[
    ("bootstrap.servers", "--brokers"),
    ("group.id", "--group"),
    (
        "enable.auto.commit",
        "the gate commits offsets itself, once the brokers acknowledge",
    ),
    (
        "enable.auto.offset.store",
        "the gate commits offsets itself, once the brokers acknowledge",
    ),
    ("enable.partition.eof", "--exit-at-end"),
    (
        "enable.idempotence",
        "the gate commits only what all in-sync replicas acknowledge",
    ),
    (
        "acks",
        "the gate commits only what all in-sync replicas acknowledge",
    ),
    (
        "request.required.acks",
        "the gate commits only what all in-sync replicas acknowledge",
    ),
];

/// Where the gate reads and writes, and how it connects.
#[derive(Debug, Clone)]
pub struct Topics {
    /// Bootstrap brokers, `host:port[,host:port…]`.
    pub brokers: String,
    /// The topic the gate consumes.
    pub from: String,
    /// The topic the records that go on are produced to.
    pub to: String,
    /// The topic dead letters are produced to. Without one they go to the
    /// writer [`run`] is given, one envelope per line.
    pub dlq: Option<String>,
    /// The consumer group the gate commits its progress under.
    pub group: String,
    /// Stop once every assigned partition has been read to its end.
    pub exit_at_end: bool,
    /// How often the gate settles: waits for its deliveries, then commits.
    /// Longer means fewer commits and more records judged again after a
    /// crash; a rebalance and the end of a run settle regardless.
    pub commit_interval: Duration,
    /// librdkafka properties for both clients, applied after the gate's own.
    pub properties: Vec<(String, String)>,
}

impl Topics {
    /// The group a gate commits under when none is named: one per contract
    /// and source topic, so two gates on different topics never share one.
    pub fn default_group(contract_id: &str, from: &str) -> String {
        format!("covenant-gate.{contract_id}.{from}")
    }

    /// Refuse what cannot work before connecting: a gate reading its own
    /// output would read it forever, and the properties at-least-once rests
    /// on are the gate's to set.
    fn validate(&self) -> Result<()> {
        let usage = |message: String| Err(CovenantError::Usage { message });
        if self.to == self.from {
            return usage(format!("--to {} is the topic the gate reads", self.to));
        }
        if let Some(dlq) = &self.dlq {
            if dlq == &self.from || dlq == &self.to {
                return usage(format!(
                    "--dlq-topic {dlq} must differ from --from and --to"
                ));
            }
        }
        for (key, _) in &self.properties {
            if let Some((_, why)) = OWN_PROPERTIES.iter().find(|(own, _)| own == key) {
                return usage(format!("-X {key} is not settable ({why})"));
            }
        }
        Ok(())
    }
}

/// Parse `key=value` librdkafka properties: `-X` arguments, or the lines of
/// a properties file (blank lines and `#` or `!` comments skipped).
pub fn parse_properties(lines: &str, origin: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for (n, line) in lines.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        match line.split_once('=') {
            Some((key, value)) if !key.trim().is_empty() => {
                out.push((key.trim().to_string(), value.trim().to_string()))
            }
            _ => {
                return Err(CovenantError::Usage {
                    message: format!("{origin}:{}: expected key=value, got {line:?}", n + 1),
                })
            }
        }
    }
    Ok(out)
}

/// A finished Kafka gate run.
#[derive(Debug)]
pub struct KafkaOutcome {
    pub stats: GateStats,
    /// True when enforcement (policy `block`, minus any `max_violations`
    /// budget) says the stream failed.
    pub failed: bool,
    /// Records with no value, passed on unjudged.
    pub tombstones: u64,
}

fn transport(message: String) -> CovenantError {
    CovenantError::Transport {
        transport: "kafka".to_string(),
        message,
    }
}

/// Run the gate over `topics` until `stop` is set (or, with
/// `exit_at_end`, until every assigned partition is read to its end).
/// Dead letters go to the DLQ topic, or to `dlq` when there is none. Every
/// violation is counted into `report`, when given, for the run's document.
#[allow(clippy::too_many_arguments)]
pub fn run(
    contract: &CompiledContract,
    model: &CompiledModel,
    topics: &Topics,
    dlq: Box<dyn Write + Send>,
    track_unique: bool,
    mut sink: Option<&mut StatsSink>,
    mut report: Option<&mut Collector>,
    stop: &AtomicBool,
) -> Result<KafkaOutcome> {
    topics.validate()?;
    let producer: BaseProducer<Deliveries> = config(topics, false)
        .create_with_context(Deliveries::default())
        .map_err(|e| transport(format!("cannot create the producer: {e}")))?;
    let settle = Arc::new(Settle {
        from: topics.from.clone(),
        producer,
        dlq: Mutex::new(dlq),
        pending: Mutex::default(),
        failure: Mutex::default(),
    });
    let consumer: BaseConsumer<GateContext> = config(topics, true)
        .create_with_context(GateContext {
            settle: Arc::clone(&settle),
            rebalances: AtomicU64::new(0),
        })
        .map_err(|e| transport(format!("cannot create the consumer: {e}")))?;

    check_topics(&consumer, topics)?;
    consumer
        .subscribe(&[topics.from.as_str()])
        .map_err(|e| transport(format!("cannot subscribe to {}: {e}", topics.from)))?;

    let mut gate = RecordGate::new(contract, model, track_unique);
    let mut tombstones = 0u64;
    // The next offset not yet handled, per partition, for this whole run.
    let mut handled: HashMap<i32, i64> = HashMap::new();
    let mut at_end: HashSet<i32> = HashSet::new();
    let mut rebalances = 0;
    let mut last_settle = Instant::now();
    let mut warned: Option<(RDKafkaErrorCode, Instant)> = None;

    while !stop.load(Ordering::SeqCst) {
        if let Some(failure) = settle.failure.lock().unwrap().take() {
            return Err(transport(failure));
        }
        settle.producer.poll(Duration::ZERO);
        match consumer.poll(POLL) {
            None => {}
            Some(Ok(msg)) => {
                let partition = msg.partition();
                at_end.remove(&partition);
                // Read before (a partition handed back after its commit
                // failed): already judged and produced, so not again.
                if handled
                    .get(&partition)
                    .is_some_and(|&next| msg.offset() < next)
                {
                    continue;
                }
                let judged = handle(&msg, &mut gate, &settle, topics, sink.is_some())?;
                handled.insert(partition, msg.offset() + 1);
                settle
                    .pending
                    .lock()
                    .unwrap()
                    .insert(partition, msg.offset() + 1);
                match judged {
                    None => tombstones += 1,
                    Some((verdict, validate_ns)) => {
                        count_into(report.as_deref_mut(), gate.violations());
                        if let Some(s) = sink.as_deref_mut() {
                            s.on_record(verdict == Verdict::Block, gate.violations(), validate_ns);
                            s.maybe_write(gate.stats());
                        }
                    }
                }
            }
            Some(Err(KafkaError::PartitionEOF(partition))) => {
                // A rebalance since the last end counts as a fresh start.
                let now = consumer.context().rebalances.load(Ordering::SeqCst);
                if now != rebalances {
                    rebalances = now;
                    at_end.clear();
                }
                at_end.insert(partition);
                if topics.exit_at_end && read_to_end(&consumer, &at_end) {
                    break;
                }
            }
            Some(Err(e)) => consumer_error(e, &mut warned)?,
        }
        if last_settle.elapsed() >= topics.commit_interval {
            settle.settle(&consumer).map_err(transport)?;
            last_settle = Instant::now();
        }
    }
    settle.settle(&consumer).map_err(transport)?;
    if let Some(failure) = settle.failure.lock().unwrap().take() {
        return Err(transport(failure));
    }
    if let Some(s) = sink {
        s.write(gate.stats());
    }
    let failed = gate.failed();
    Ok(KafkaOutcome {
        stats: gate.into_stats(),
        failed,
        tombstones,
    })
}

/// Judge one message and produce what follows from the verdict: the verdict
/// and, when `timed`, how long the judging took (the stats sidecar's added
/// latency). `None` for a tombstone, which goes on unjudged.
fn handle(
    msg: &BorrowedMessage<'_>,
    gate: &mut RecordGate<'_>,
    settle: &Settle,
    topics: &Topics,
    timed: bool,
) -> Result<Option<(Verdict, Option<u64>)>> {
    // The source's headers, copied natively: rdkafka's header iterator
    // panics on a key that is not UTF-8, and one odd producer must not stop
    // the gate for every record behind it.
    let headers = || msg.headers().map(|h| h.detach());
    let Some(value) = msg.payload() else {
        let record = forward(msg, &topics.to, None, headers());
        settle.produce(record).map_err(transport)?;
        return Ok(None);
    };

    let t0 = timed.then(Instant::now);
    let verdict = gate.judge(value);
    let validate_ns = t0.map(|t| t.elapsed().as_nanos() as u64);
    if verdict != Verdict::Pass {
        let mut letter = serde_json::to_vec(&gate.dead_letter())
            .map_err(|e| transport(format!("cannot serialize a dead letter: {e}")))?;
        match &topics.dlq {
            Some(dlq) => {
                let offset = msg.offset().to_string();
                let partition = msg.partition().to_string();
                let coordinates = headers()
                    .unwrap_or_else(OwnedHeaders::new)
                    .insert(Header {
                        key: "covenant.source.topic",
                        value: Some(msg.topic()),
                    })
                    .insert(Header {
                        key: "covenant.source.partition",
                        value: Some(partition.as_str()),
                    })
                    .insert(Header {
                        key: "covenant.source.offset",
                        value: Some(offset.as_str()),
                    });
                // The dead letter's own time, not the record's: a DLQ keeps
                // what was rejected for as long as its retention says from
                // the rejection, even in a backfill of old records.
                let mut record = BaseRecord::<[u8], [u8]>::to(dlq)
                    .payload(&letter)
                    .headers(coordinates);
                if let Some(key) = msg.key() {
                    record = record.key(key);
                }
                settle.produce(record).map_err(transport)?;
            }
            None => {
                letter.push(b'\n');
                settle
                    .dlq
                    .lock()
                    .unwrap()
                    .write_all(&letter)
                    .map_err(|e| CovenantError::Io {
                        path: "<dlq>".to_string(),
                        source: e,
                    })?;
            }
        }
    }
    if verdict != Verdict::Block {
        let record = forward(msg, &topics.to, Some(value), headers());
        settle.produce(record).map_err(transport)?;
    }
    Ok(Some((verdict, validate_ns)))
}

/// The record as it arrived — key, value, headers and timestamp — addressed
/// to `to`.
fn forward<'a>(
    msg: &'a BorrowedMessage<'_>,
    to: &'a str,
    value: Option<&'a [u8]>,
    headers: Option<OwnedHeaders>,
) -> BaseRecord<'a, [u8], [u8]> {
    let mut record = BaseRecord::<[u8], [u8]>::to(to);
    if let Some(value) = value {
        record = record.payload(value);
    }
    if let Some(key) = msg.key() {
        record = record.key(key);
    }
    if let Some(headers) = headers {
        record = record.headers(headers);
    }
    if let Some(ts) = msg.timestamp().to_millis() {
        record = record.timestamp(ts);
    }
    record
}

fn config(topics: &Topics, consumer: bool) -> ClientConfig {
    let mut config = ClientConfig::new();
    config
        .set("bootstrap.servers", &topics.brokers)
        .set("client.id", "covenant-gate");
    if consumer {
        config
            .set("group.id", &topics.group)
            .set("enable.auto.commit", "false")
            .set("enable.auto.offset.store", "false")
            // A new group starts at the beginning: a gate that skipped what
            // was already in the topic would pass it unjudged.
            .set("auto.offset.reset", "earliest")
            .set("enable.partition.eof", topics.exit_at_end.to_string());
    } else {
        config
            // A record counts as delivered once all in-sync replicas have it,
            // and a retry never writes it twice.
            .set("enable.idempotence", "true")
            .set("acks", "all")
            // The Java client's partitioner, so a key lands in the partition
            // a Java producer would have put it in.
            .set("partitioner", "murmur2_random");
    }
    for (key, value) in &topics.properties {
        config.set(key, value);
    }
    config
}

/// Before the first record: the brokers answer, and every topic the gate
/// names exists. The gate never creates topics, so a mistyped name is an
/// error here rather than a new topic with a broker's default settings.
fn check_topics(consumer: &BaseConsumer<GateContext>, topics: &Topics) -> Result<()> {
    let named = [Some(&topics.from), Some(&topics.to), topics.dlq.as_ref()];
    for topic in named.into_iter().flatten() {
        let metadata = consumer
            .fetch_metadata(Some(topic), METADATA_TIMEOUT)
            .map_err(|e| {
                transport(format!(
                    "no answer from the brokers ({}) about topic {topic}: {e}",
                    topics.brokers
                ))
            })?;
        let found = metadata.topics().iter().find(|t| t.name() == topic);
        match found.map(|t| (t.error(), t.partitions().len())) {
            Some((None, n)) if n > 0 => {}
            Some((Some(err), _))
                if RDKafkaErrorCode::from(err) != RDKafkaErrorCode::UnknownTopicOrPartition =>
            {
                return Err(transport(format!(
                    "topic {topic}: {}",
                    RDKafkaErrorCode::from(err)
                )));
            }
            _ => {
                return Err(transport(format!(
                    "topic {topic} does not exist; create it first (the gate never creates topics)"
                )))
            }
        }
    }
    Ok(())
}

/// Every partition assigned to this gate has been read to its end.
fn read_to_end(consumer: &BaseConsumer<GateContext>, at_end: &HashSet<i32>) -> bool {
    consumer.assignment().is_ok_and(|assigned| {
        let assigned = assigned.elements();
        !assigned.is_empty() && assigned.iter().all(|p| at_end.contains(&p.partition()))
    })
}

/// A consumer error: fatal when no retry can mend it, otherwise said on
/// stderr (at most every [`WARN_EVERY`] for the same error) while the
/// client reconnects.
fn consumer_error(e: KafkaError, warned: &mut Option<(RDKafkaErrorCode, Instant)>) -> Result<()> {
    let code = match &e {
        KafkaError::MessageConsumptionFatal(_) => return Err(transport(e.to_string())),
        KafkaError::MessageConsumption(code) => *code,
        other => return Err(transport(other.to_string())),
    };
    if matches!(
        code,
        RDKafkaErrorCode::UnknownTopicOrPartition
            | RDKafkaErrorCode::UnknownTopic
            | RDKafkaErrorCode::TopicAuthorizationFailed
            | RDKafkaErrorCode::GroupAuthorizationFailed
            | RDKafkaErrorCode::ClusterAuthorizationFailed
            | RDKafkaErrorCode::SaslAuthenticationFailed
            | RDKafkaErrorCode::Authentication
    ) {
        return Err(transport(format!("consuming stopped: {code}")));
    }
    let repeat = warned
        .as_ref()
        .is_some_and(|(last, at)| *last == code && at.elapsed() < WARN_EVERY);
    if !repeat {
        eprintln!("covenant gate: kafka: {code} (retrying)");
        *warned = Some((code, Instant::now()));
    }
    Ok(())
}

/// What must be settled before an offset is committed, shared by the main
/// loop and the rebalance callback.
struct Settle {
    from: String,
    producer: BaseProducer<Deliveries>,
    /// Dead letters, when they go to a writer rather than a topic.
    dlq: Mutex<Box<dyn Write + Send>>,
    /// The offsets to commit: per partition, the next one not yet handled.
    pending: Mutex<HashMap<i32, i64>>,
    /// A settle that failed where no error can be returned (a rebalance).
    failure: Mutex<Option<String>>,
}

impl Settle {
    /// Produce, waiting for room when the producer's queue is full.
    fn produce(&self, mut record: BaseRecord<'_, [u8], [u8]>) -> std::result::Result<(), String> {
        loop {
            match self.producer.send(record) {
                Ok(()) => return Ok(()),
                Err((KafkaError::MessageProduction(RDKafkaErrorCode::QueueFull), back)) => {
                    record = back;
                    self.producer.poll(POLL);
                }
                Err((KafkaError::MessageProduction(RDKafkaErrorCode::MessageSizeTooLarge), back)) => {
                    return Err(format!(
                        "a record for {} is larger than message.max.bytes; raise it with -X message.max.bytes=… (and the topic's max.message.bytes)",
                        back.topic
                    ))
                }
                Err((e, back)) => return Err(format!("cannot produce to {}: {e}", back.topic)),
            }
        }
    }

    /// Wait until the brokers have acknowledged everything produced and the
    /// dead-letter writer is flushed, then commit what has been handled.
    /// Nothing is committed if anything failed to arrive.
    fn settle(&self, consumer: &BaseConsumer<GateContext>) -> std::result::Result<(), String> {
        self.producer
            .flush(FLUSH_TIMEOUT)
            .map_err(|e| format!("the brokers did not acknowledge every record: {e}"))?;
        if let Some(failed) = self.producer.context().failed.lock().unwrap().clone() {
            return Err(failed);
        }
        self.dlq
            .lock()
            .unwrap()
            .flush()
            .map_err(|e| format!("cannot write dead letters: {e}"))?;
        let mut pending = self.pending.lock().unwrap();
        if pending.is_empty() {
            return Ok(());
        }
        let mut offsets = TopicPartitionList::new();
        for (&partition, &next) in pending.iter() {
            offsets
                .add_partition_offset(&self.from, partition, Offset::Offset(next))
                .map_err(|e| format!("cannot commit: {e}"))?;
        }
        match consumer.commit(&offsets, CommitMode::Sync) {
            Ok(()) => pending.clear(),
            // Not fatal: everything is delivered, so a lost commit only
            // means these records are read again after a restart.
            Err(e) => eprintln!("covenant gate: kafka: commit failed, will retry: {e}"),
        }
        Ok(())
    }
}

/// The consumer's callbacks: settle before partitions are taken away, so the
/// next owner starts exactly where this gate stopped.
struct GateContext {
    settle: Arc<Settle>,
    rebalances: AtomicU64,
}

impl ClientContext for GateContext {}

impl ConsumerContext for GateContext {
    fn pre_rebalance(&self, consumer: &BaseConsumer<Self>, rebalance: &Rebalance<'_>) {
        match rebalance {
            Rebalance::Revoke(revoked) => {
                if let Err(e) = self.settle.settle(consumer) {
                    self.settle.failure.lock().unwrap().get_or_insert(e);
                }
                // Revoked partitions are another gate's now, committed or not.
                let mut pending = self.settle.pending.lock().unwrap();
                for p in revoked.elements() {
                    pending.remove(&p.partition());
                }
            }
            Rebalance::Assign(_) => {}
            Rebalance::Error(e) => eprintln!("covenant gate: kafka: rebalance failed: {e}"),
        }
    }

    fn post_rebalance(&self, _: &BaseConsumer<Self>, _: &Rebalance<'_>) {
        self.rebalances.fetch_add(1, Ordering::SeqCst);
    }
}

/// The producer's callbacks: the first delivery that failed, kept so that
/// nothing after it is committed.
#[derive(Default)]
struct Deliveries {
    failed: Mutex<Option<String>>,
}

impl ClientContext for Deliveries {
    fn error(&self, error: KafkaError, reason: &str) {
        eprintln!("covenant gate: kafka producer: {reason} ({error})");
    }
}

impl ProducerContext for Deliveries {
    type DeliveryOpaque = ();

    fn delivery(&self, result: &DeliveryResult<'_>, _: ()) {
        if let Err((e, msg)) = result {
            self.failed.lock().unwrap().get_or_insert_with(|| {
                format!("a record for {} was not delivered: {e}", msg.topic())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topics() -> Topics {
        Topics {
            brokers: "localhost:9092".to_string(),
            from: "raw".to_string(),
            to: "clean".to_string(),
            dlq: Some("dlq".to_string()),
            group: Topics::default_group("orders", "raw"),
            exit_at_end: false,
            commit_interval: Duration::from_secs(1),
            properties: Vec::new(),
        }
    }

    #[test]
    fn properties_parse_as_the_java_client_writes_them() {
        let parsed = parse_properties(
            "# a comment\n! another\n\nsecurity.protocol = SASL_SSL\nsasl.password=a=b\n",
            "client.properties",
        )
        .unwrap();
        assert_eq!(
            parsed,
            vec![
                ("security.protocol".to_string(), "SASL_SSL".to_string()),
                ("sasl.password".to_string(), "a=b".to_string()),
            ]
        );
        let err = parse_properties("no equals sign", "-X").unwrap_err();
        assert!(
            err.to_string().contains("-X:1: expected key=value"),
            "{err}"
        );
    }

    #[test]
    fn a_gate_never_reads_its_own_output() {
        let mut t = topics();
        t.to = "raw".to_string();
        assert!(t.validate().is_err());
        let mut t = topics();
        t.dlq = Some("clean".to_string());
        assert!(t.validate().is_err());
        assert!(topics().validate().is_ok());
    }

    #[test]
    fn the_properties_at_least_once_rests_on_are_the_gates() {
        for key in [
            "enable.auto.commit",
            "group.id",
            "bootstrap.servers",
            "enable.idempotence",
            "acks",
            "request.required.acks",
        ] {
            let mut t = topics();
            t.properties = vec![(key.to_string(), "x".to_string())];
            let err = t.validate().unwrap_err().to_string();
            assert!(err.contains(key), "{err}");
        }
        let mut t = topics();
        t.properties = vec![("security.protocol".to_string(), "SSL".to_string())];
        assert!(t.validate().is_ok());
    }

    #[test]
    fn the_default_group_names_the_contract_and_the_topic() {
        assert_eq!(
            Topics::default_group("orders", "orders.raw"),
            "covenant-gate.orders.orders.raw"
        );
    }
}
