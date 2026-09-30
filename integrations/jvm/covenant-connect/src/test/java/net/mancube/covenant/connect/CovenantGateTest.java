package net.mancube.covenant.connect;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.Map;
import org.apache.kafka.common.config.ConfigException;
import org.apache.kafka.connect.data.Schema;
import org.apache.kafka.connect.data.SchemaBuilder;
import org.apache.kafka.connect.data.Struct;
import org.apache.kafka.connect.errors.DataException;
import org.apache.kafka.connect.source.SourceRecord;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class CovenantGateTest {
  static final String ORDERS = Path.of("../../../examples/contracts/orders.yaml").toString();
  static final String CLEAN =
      "{\"order_id\":\"ord_a1b2c3d4e5f6\",\"amount_cents\":1,\"currency\":\"USD\","
          + "\"created_at\":\"2026-08-11T09:30:00Z\"}";

  final CovenantGate<SourceRecord> smt = new CovenantGate<>();

  @AfterEach
  void close() {
    smt.close();
  }

  static SourceRecord record(Schema schema, Object value) {
    return new SourceRecord(Map.of(), Map.of(), "orders_raw", 0, Schema.STRING_SCHEMA, "k", schema, value);
  }

  void configure(String contract, String... pairs) {
    Map<String, String> props = new HashMap<>(Map.of(CovenantGate.CONTRACT, contract));
    for (int i = 0; i < pairs.length; i += 2) {
      props.put(pairs[i], pairs[i + 1]);
    }
    smt.configure(props);
  }

  @Test
  void aRecordThatKeepsTheContractGoesOnUnchanged() {
    configure(ORDERS, "unique", "skip");
    SourceRecord clean = record(Schema.STRING_SCHEMA, CLEAN);
    assertSame(clean, smt.apply(clean));
  }

  @Test
  void aBlockedRecordFailsWithItsDeadLetter() {
    configure(ORDERS, "unique", "skip");
    DataException e =
        assertThrows(DataException.class, () -> smt.apply(record(Schema.STRING_SCHEMA, "{\"order_id\":\"nope\"}")));
    assertTrue(e.getMessage().startsWith("{\"contract_id\":\"orders\""), e.getMessage());
    assertTrue(e.getMessage().contains("\"rule\":\"pattern\""), e.getMessage());
  }

  @Test
  void onBlockDropDropsIt() {
    configure(ORDERS, "unique", "skip", "on.block", "drop");
    assertNull(smt.apply(record(Schema.STRING_SCHEMA, "not json")));
  }

  @Test
  void aMapOrAStructIsJudgedAsJson() {
    configure(ORDERS, "unique", "skip");
    Map<String, Object> map = new HashMap<>();
    map.put("order_id", "ord_a1b2c3d4e5f6");
    map.put("amount_cents", 1);
    map.put("currency", "USD");
    map.put("created_at", "2026-08-11T09:30:00Z");
    SourceRecord schemaless = record(null, map);
    assertSame(schemaless, smt.apply(schemaless));

    Schema schema =
        SchemaBuilder.struct()
            .field("order_id", Schema.STRING_SCHEMA)
            .field("amount_cents", Schema.INT64_SCHEMA)
            .field("currency", Schema.STRING_SCHEMA)
            .field("created_at", Schema.STRING_SCHEMA)
            .build();
    Struct bad =
        new Struct(schema)
            .put("order_id", "ord_a1b2c3d4e5f6")
            .put("amount_cents", -5L)
            .put("currency", "USD")
            .put("created_at", "2026-08-11T09:30:00Z");
    DataException e = assertThrows(DataException.class, () -> smt.apply(record(schema, bad)));
    assertTrue(e.getMessage().contains("\"rule\":\"min\""), e.getMessage());
  }

  @Test
  void aTombstoneGoesOnUnjudged() {
    configure(ORDERS, "unique", "skip");
    SourceRecord tombstone = record(null, null);
    assertSame(tombstone, smt.apply(tombstone));
  }

  @Test
  void underWarnTheRecordCarriesItsDeadLetter(@TempDir Path tmp) throws Exception {
    Path warn = tmp.resolve("orders-warn.yaml");
    Files.writeString(
        warn, Files.readString(Path.of(ORDERS)).replace("on_violation: block", "on_violation: warn"));
    configure(warn.toString(), "unique", "skip");
    SourceRecord out = smt.apply(record(Schema.STRING_SCHEMA, "{\"order_id\":\"nope\"}"));
    assertEquals("{\"order_id\":\"nope\"}", out.value());
    String letter = (String) out.headers().lastWithName(CovenantGate.DEAD_LETTER_HEADER).value();
    assertTrue(letter.contains("\"rule\":\"pattern\""), letter);
  }

  @Test
  void aContractThatCannotBeEnforcedIsAConfigError() {
    ConfigException e = assertThrows(ConfigException.class, () -> configure(ORDERS));
    assertTrue(e.getMessage().contains("declares unique fields (order_id)"), e.getMessage());
    assertThrows(ConfigException.class, () -> configure("/no/such/contract.yaml", "unique", "skip"));
    assertThrows(ConfigException.class, () -> configure(ORDERS, "unique", "sometimes"));
  }
}
