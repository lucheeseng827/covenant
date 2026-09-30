package net.mancube.covenant.connect;

import net.mancube.covenant.CovenantException;
import net.mancube.covenant.Gate;
import net.mancube.covenant.UniqueKeys;
import net.mancube.covenant.Verdict;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.Locale;
import java.util.Map;
import org.apache.kafka.common.config.AbstractConfig;
import org.apache.kafka.common.config.ConfigDef;
import org.apache.kafka.common.config.ConfigException;
import org.apache.kafka.connect.connector.ConnectRecord;
import org.apache.kafka.connect.errors.DataException;
import org.apache.kafka.connect.header.Headers;
import org.apache.kafka.connect.json.JsonConverter;
import org.apache.kafka.connect.json.JsonConverterConfig;
import org.apache.kafka.connect.transforms.Transformation;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Enforces a Covenant data contract on every record's value, with the engine {@code covenant
 * gate} runs.
 *
 * <ul>
 *   <li>A record that keeps the contract goes on unchanged.
 *   <li>A record that breaks it under {@code on_violation: block} fails with a {@link
 *       DataException} whose message is its dead letter, the envelope {@code covenant gate}
 *       writes. Connect's own error handling decides what happens next: by default the task
 *       stops; with {@code errors.tolerance=all} and {@code errors.deadletterqueue.topic.name} a
 *       sink connector routes the record to that topic, with the envelope in the {@code
 *       __connect.errors.exception.message} header. With {@code on.block=drop} it is dropped
 *       instead, and its dead letter logged.
 *   <li>A record that breaks it under {@code on_violation: warn} goes on, carrying its dead letter
 *       in a {@code covenant.dead_letter} header.
 *   <li>A record with no value, a tombstone, goes on unjudged.
 * </ul>
 *
 * <p>The value is judged as JSON: a {@code String} or {@code byte[]} value is the JSON text
 * itself (the {@code StringConverter} or {@code ByteArrayConverter}), and anything else (a {@code
 * Map} or {@code Struct}) is rendered to JSON first.
 */
public class CovenantGate<R extends ConnectRecord<R>> implements Transformation<R> {
  public static final String CONTRACT = "contract";
  public static final String MODEL = "model";
  public static final String UNIQUE = "unique";
  public static final String ON_BLOCK = "on.block";
  /** The header a warned record carries its dead letter in. */
  public static final String DEAD_LETTER_HEADER = "covenant.dead_letter";

  private static final Logger LOG = LoggerFactory.getLogger(CovenantGate.class);

  static final ConfigDef CONFIG_DEF =
      new ConfigDef()
          .define(
              CONTRACT,
              ConfigDef.Type.STRING,
              ConfigDef.NO_DEFAULT_VALUE,
              ConfigDef.Importance.HIGH,
              "Path to the contract, covenant: 1 or ODCS v3, on the worker.")
          .define(
              MODEL,
              ConfigDef.Type.STRING,
              null,
              ConfigDef.Importance.MEDIUM,
              "The model to enforce, when the contract has more than one.")
          .define(
              UNIQUE,
              ConfigDef.Type.STRING,
              "refuse",
              ConfigDef.ValidString.in("refuse", "skip", "track"),
              ConfigDef.Importance.MEDIUM,
              "The model's unique fields: refuse a model that declares any, skip them, or track"
                  + " them across what this task judges. A task sees only its own partitions and"
                  + " starts afresh when it restarts, so tracking holds unique only within that.")
          .define(
              ON_BLOCK,
              ConfigDef.Type.STRING,
              "fail",
              ConfigDef.ValidString.in("fail", "drop"),
              ConfigDef.Importance.MEDIUM,
              "A blocked record: fail with its dead letter, for Connect's error handling (and a"
                  + " sink's dead letter queue), or drop it and log its dead letter.");

  private Gate gate;
  private JsonConverter json;
  private boolean drop;

  @Override
  public void configure(Map<String, ?> props) {
    AbstractConfig config = new AbstractConfig(CONFIG_DEF, props);
    Path contract = Path.of(config.getString(CONTRACT));
    UniqueKeys unique = UniqueKeys.valueOf(config.getString(UNIQUE).toUpperCase(Locale.ROOT));
    try {
      gate = Gate.open(contract, config.getString(MODEL), unique);
    } catch (IOException e) {
      throw new ConfigException(CONTRACT, contract.toString(), "cannot read it: " + e);
    } catch (CovenantException e) {
      throw new ConfigException(CONTRACT, contract.toString(), e.getMessage());
    }
    json = new JsonConverter();
    json.configure(Map.of(JsonConverterConfig.SCHEMAS_ENABLE_CONFIG, false), false);
    drop = "drop".equals(config.getString(ON_BLOCK));
  }

  @Override
  public R apply(R record) {
    Object value = record.value();
    if (value == null) {
      return record;
    }
    Verdict verdict = gate.judge(json(record, value));
    switch (verdict) {
      case PASS:
        return record;
      case WARN:
        Headers headers = record.headers().duplicate();
        headers.addString(DEAD_LETTER_HEADER, gate.deadLetter());
        return record.newRecord(
            record.topic(),
            record.kafkaPartition(),
            record.keySchema(),
            record.key(),
            record.valueSchema(),
            record.value(),
            record.timestamp(),
            headers);
      default:
        String letter = gate.deadLetter();
        if (drop) {
          LOG.warn("covenant: dropped a record that breaks the contract: {}", letter);
          return null;
        }
        throw new DataException(letter);
    }
  }

  /** The value as the JSON text the gate judges. */
  private byte[] json(R record, Object value) {
    if (value instanceof byte[] bytes) {
      return bytes;
    }
    if (value instanceof String text) {
      return text.getBytes(StandardCharsets.UTF_8);
    }
    return json.fromConnectData(record.topic(), record.valueSchema(), value);
  }

  @Override
  public ConfigDef config() {
    return CONFIG_DEF;
  }

  @Override
  public void close() {
    if (gate != null) {
      gate.close();
    }
    if (json != null) {
      json.close();
    }
  }
}
