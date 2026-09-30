package net.mancube.covenant;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.Assumptions;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

class GateTest {
  /** The engine's demo contract and data, beside this module in the source tree. */
  static final Path EXAMPLES = Path.of("../../../examples");
  static final Path ORDERS = EXAMPLES.resolve("contracts/orders.yaml");

  static List<String> demoRecords() throws IOException {
    List<String> records = new ArrayList<>();
    for (String name : List.of("orders.ndjson", "orders_bad.ndjson")) {
      for (String line : Files.readAllLines(EXAMPLES.resolve("data/" + name))) {
        if (!line.isBlank()) {
          records.add(line);
        }
      }
    }
    return records;
  }

  static String withoutTs(String letter) {
    return letter.replaceFirst("\"ts\":\"[^\"]*\",", "");
  }

  @Test
  void aRecordIsJudgedAndADeadLetterExplainsIt() throws IOException {
    try (Gate gate = Gate.open(ORDERS, null, UniqueKeys.SKIP)) {
      String clean =
          "{\"order_id\":\"ord_a1b2c3d4e5f6\",\"amount_cents\":1,\"currency\":\"USD\","
              + "\"created_at\":\"2026-08-11T09:30:00Z\"}";
      assertEquals(Verdict.PASS, gate.judge(clean));
      assertNull(gate.deadLetter());

      assertEquals(Verdict.BLOCK, gate.judge("{\"order_id\":\"nope\"}"));
      String letter = gate.deadLetter();
      assertTrue(letter.startsWith("{\"contract_id\":\"orders\",\"contract_version\":\"1.2.0\""), letter);
      assertTrue(letter.contains("\"rule\":\"pattern\""), letter);
      assertTrue(letter.contains("\"record\":{\"order_id\":\"nope\"}"), letter);

      assertEquals(Verdict.BLOCK, gate.judge("not json"));
      assertTrue(gate.deadLetter().contains("\"record\":\"not json\""));

      assertEquals(new Stats(3, 1, 2, 0, true), gate.stats());
    }
  }

  @Test
  void aModelWithUniqueFieldsIsRefusedUnlessTheCallerChooses() {
    CovenantException refused =
        assertThrows(CovenantException.class, () -> Gate.open(ORDERS, null, UniqueKeys.REFUSE));
    assertTrue(refused.getMessage().contains("declares unique fields (order_id)"), refused.getMessage());
  }

  @Test
  void trackedKeysSpanTheRecordsAGateJudges() throws IOException {
    String record =
        "{\"order_id\":\"ord_a1b2c3d4e5f6\",\"amount_cents\":1,\"currency\":\"USD\","
            + "\"created_at\":\"2026-08-11T09:30:00Z\"}";
    try (Gate gate = Gate.open(ORDERS, null, UniqueKeys.TRACK)) {
      assertEquals(Verdict.PASS, gate.judge(record));
      assertEquals(Verdict.BLOCK, gate.judge(record));
      assertTrue(gate.deadLetter().contains("\"rule\":\"unique\""));
    }
    // Each gate keeps its own keys.
    try (Gate other = Gate.open(ORDERS, null, UniqueKeys.TRACK)) {
      assertEquals(Verdict.PASS, other.judge(record));
    }
  }

  @Test
  void aContractThatCannotBeEnforcedIsRefusedWithItsReason() {
    CovenantException e =
        assertThrows(
            CovenantException.class,
            () ->
                Gate.open(
                    "covenant: 1\nid: x\nversion: nope\nmodels: {}\n", "x.yaml", null, UniqueKeys.SKIP));
    assertTrue(e.getMessage().contains("not valid semver"), e.getMessage());
  }

  @Test
  void aRecordLargerThanTheBufferGrowsIt() throws IOException {
    String big = "{\"order_id\":\"" + "x".repeat(100_000) + "\"}";
    try (Gate gate = Gate.open(ORDERS, null, UniqueKeys.SKIP)) {
      assertEquals(Verdict.BLOCK, gate.judge(big));
      assertTrue(gate.deadLetter().contains("\"rule\":\"pattern\""));
    }
  }

  @Test
  void aClosedGateSaysSo() throws IOException {
    Gate gate = Gate.open(ORDERS, null, UniqueKeys.SKIP);
    gate.close();
    assertThrows(IllegalStateException.class, () -> gate.judge("{}"));
  }

  /**
   * The same verdicts and dead letters as the command, on the demo data, when the build is told
   * where the native binary is ({@code -Dcovenant.bin=…}).
   */
  @Test
  void theGateDecidesAsCovenantGateDoes(@TempDir Path tmp) throws Exception {
    String bin = System.getProperty("covenant.bin");
    Assumptions.assumeTrue(bin != null && !bin.isEmpty(), "set -Dcovenant.bin to compare");
    List<String> records = demoRecords();
    for (UniqueKeys unique : List.of(UniqueKeys.SKIP, UniqueKeys.TRACK)) {
      Path input = tmp.resolve("in.ndjson");
      Path dlq = tmp.resolve(unique + ".dlq.ndjson");
      Files.write(input, records);
      List<String> command = new ArrayList<>(List.of(bin, "gate", "-c", ORDERS.toString(), "-q"));
      command.addAll(List.of("--dlq", dlq.toString()));
      if (unique == UniqueKeys.SKIP) {
        command.add("--no-unique");
      }
      Process process =
          new ProcessBuilder(command).redirectInput(input.toFile()).redirectErrorStream(false).start();
      List<String> passedByCommand =
          new String(process.getInputStream().readAllBytes()).lines().toList();
      assertTrue(process.waitFor(60, TimeUnit.SECONDS));

      List<String> passed = new ArrayList<>();
      List<String> letters = new ArrayList<>();
      try (Gate gate = Gate.open(ORDERS, null, unique)) {
        for (String record : records) {
          Verdict verdict = gate.judge(record);
          if (verdict != Verdict.BLOCK) {
            passed.add(record);
          }
          if (verdict != Verdict.PASS) {
            letters.add(withoutTs(gate.deadLetter()));
          }
        }
      }
      assertEquals(passedByCommand, passed, unique + ": the records that go on");
      assertEquals(
          Files.readAllLines(dlq).stream().map(GateTest::withoutTs).toList(),
          letters,
          unique + ": the dead letters");
    }
  }
}
