package net.mancube.covenant.kroxylicious;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import net.mancube.covenant.Gate;
import net.mancube.covenant.UniqueKeys;
import org.apache.kafka.common.Uuid;
import org.apache.kafka.common.compress.Compression;
import org.apache.kafka.common.message.ProduceRequestData;
import org.apache.kafka.common.message.ProduceRequestData.PartitionProduceData;
import org.apache.kafka.common.message.ProduceRequestData.TopicProduceData;
import org.apache.kafka.common.message.ProduceResponseData;
import org.apache.kafka.common.message.ProduceResponseData.PartitionProduceResponse;
import org.apache.kafka.common.message.ProduceResponseData.TopicProduceResponse;
import org.apache.kafka.common.protocol.Errors;
import org.apache.kafka.common.record.MemoryRecords;
import org.apache.kafka.common.record.SimpleRecord;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

class JudgementTest {
  static final String CLEAN =
      "{\"order_id\":\"ord_a1b2c3d4e5f6\",\"amount_cents\":1,\"currency\":\"USD\","
          + "\"created_at\":\"2026-08-11T09:30:00Z\"}";
  static final String BAD = "{\"order_id\":\"nope\"}";

  Gate gate;

  @BeforeEach
  void open() throws IOException {
    gate = Gate.open(Path.of("../../../examples/contracts/orders.yaml"), null, UniqueKeys.SKIP);
  }

  @AfterEach
  void close() {
    gate.close();
  }

  static PartitionProduceData partition(int index, String... values) {
    SimpleRecord[] records = new SimpleRecord[values.length];
    for (int i = 0; i < values.length; i++) {
      byte[] value = values[i] == null ? null : values[i].getBytes(StandardCharsets.UTF_8);
      records[i] = new SimpleRecord(0L, null, value);
    }
    return new PartitionProduceData().setIndex(index).setRecords(MemoryRecords.withRecords(Compression.NONE, records));
  }

  static ProduceRequestData request(TopicProduceData... topics) {
    ProduceRequestData request = new ProduceRequestData().setAcks((short) -1);
    for (TopicProduceData topic : topics) {
      request.topicData().add(topic);
    }
    return request;
  }

  @Test
  void aBatchThatKeepsTheContractStaysInTheRequest() {
    ProduceRequestData request =
        request(new TopicProduceData().setName("orders").setPartitionData(new java.util.ArrayList<>(List.of(partition(0, CLEAN, null)))));
    assertTrue(Judgement.judge(request, t -> true, gate).isEmpty());
    assertEquals(1, request.topicData().find("orders", Uuid.ZERO_UUID).partitionData().size());
  }

  @Test
  void aBatchWithABrokenRecordIsTakenOutAndRefusedWhole() {
    ProduceRequestData request =
        request(
            new TopicProduceData()
                .setName("orders")
                .setPartitionData(new java.util.ArrayList<>(List.of(partition(0, CLEAN, BAD, null), partition(1, CLEAN)))));
    List<Judgement.Refusal> refusals = Judgement.judge(request, t -> true, gate);

    assertEquals(1, refusals.size());
    Judgement.Refusal refusal = refusals.get(0);
    assertEquals(0, refusal.partition());
    assertEquals(1, refusal.errors().size(), "the tombstone and the clean record are not errors");
    assertEquals(1, refusal.errors().get(0).batchIndex());
    String letter = refusal.errors().get(0).batchIndexErrorMessage();
    assertTrue(letter.startsWith("{\"contract_id\":\"orders\"") && letter.contains("\"rule\":\"pattern\""), letter);
    List<PartitionProduceData> left = request.topicData().find("orders", Uuid.ZERO_UUID).partitionData();
    assertEquals(List.of(1), left.stream().map(PartitionProduceData::index).toList());
  }

  @Test
  void aTopicLeftWithoutPartitionsLeavesTheRequestAndOnlyGatedTopicsAreJudged() {
    Uuid id = Uuid.randomUuid();
    ProduceRequestData request =
        request(
            new TopicProduceData().setTopicId(id).setPartitionData(new java.util.ArrayList<>(List.of(partition(0, BAD)))),
            new TopicProduceData().setName("audit").setPartitionData(new java.util.ArrayList<>(List.of(partition(0, BAD)))));
    // Topics named by id are gated by the name their id maps to.
    Map<Uuid, String> byId = Map.of(id, "orders");
    List<Judgement.Refusal> refusals =
        Judgement.judge(request, t -> "orders".equals(t.name().isEmpty() ? byId.get(t.topicId()) : t.name()), gate);

    assertEquals(1, refusals.size());
    assertEquals(id, refusals.get(0).topicId());
    assertEquals(1, request.topicData().size());
    assertNotNull(request.topicData().find("audit", Uuid.ZERO_UUID), "an ungated topic goes on, unjudged");
  }

  @Test
  void refusalsJoinTheBrokersResponseOrStandAlone() {
    ProduceRequestData request =
        request(
            new TopicProduceData()
                .setName("orders")
                .setPartitionData(new java.util.ArrayList<>(List.of(partition(0, BAD), partition(1, CLEAN)))));
    List<Judgement.Refusal> refusals = Judgement.judge(request, t -> true, gate);

    ProduceResponseData broker = new ProduceResponseData();
    TopicProduceResponse topic = new TopicProduceResponse().setName("orders");
    topic.partitionResponses().add(new PartitionProduceResponse().setIndex(1).setBaseOffset(41));
    broker.responses().add(topic);
    Judgement.merge(broker, refusals);

    List<PartitionProduceResponse> partitions = broker.responses().find("orders", Uuid.ZERO_UUID).partitionResponses();
    assertEquals(List.of(1, 0), partitions.stream().map(PartitionProduceResponse::index).toList());
    PartitionProduceResponse refused = partitions.get(1);
    assertEquals(Errors.INVALID_RECORD.code(), refused.errorCode());
    assertEquals(-1, refused.baseOffset());
    assertEquals(0, refused.recordErrors().get(0).batchIndex());

    ProduceResponseData alone = Judgement.response(refusals);
    assertEquals(1, alone.responses().find("orders", Uuid.ZERO_UUID).partitionResponses().size());
  }
}
