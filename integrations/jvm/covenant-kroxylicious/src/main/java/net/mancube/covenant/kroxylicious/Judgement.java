package net.mancube.covenant.kroxylicious;

import java.nio.ByteBuffer;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;
import java.util.function.Predicate;
import net.mancube.covenant.Gate;
import net.mancube.covenant.Verdict;
import org.apache.kafka.common.Uuid;
import org.apache.kafka.common.message.ProduceRequestData;
import org.apache.kafka.common.message.ProduceRequestData.PartitionProduceData;
import org.apache.kafka.common.message.ProduceRequestData.TopicProduceData;
import org.apache.kafka.common.message.ProduceResponseData;
import org.apache.kafka.common.message.ProduceResponseData.BatchIndexAndErrorMessage;
import org.apache.kafka.common.message.ProduceResponseData.PartitionProduceResponse;
import org.apache.kafka.common.message.ProduceResponseData.TopicProduceResponse;
import org.apache.kafka.common.protocol.Errors;
import org.apache.kafka.common.record.MemoryRecords;
import org.apache.kafka.common.record.Record;
import org.apache.kafka.common.record.RecordBatch;

/**
 * A produce request judged, partition by partition. A partition whose records all keep the
 * contract stays in the request. A partition with a record that breaks it is taken out and
 * refused whole, as a broker refuses a batch that fails its own validation: {@code
 * INVALID_RECORD}, with one record error per broken record carrying its dead letter. A Java
 * producer fails that record with an {@code InvalidRecordException} whose message is the dead
 * letter, and the batch's other records with an error saying they shared a batch with one.
 */
final class Judgement {
  /** A partition refused, and why. */
  record Refusal(String topic, Uuid topicId, int partition, List<BatchIndexAndErrorMessage> errors) {}

  private Judgement() {}

  /**
   * Judge every record of the partitions of {@code gated} topics, take the partitions that
   * break the contract out of {@code request} (and topics left with none), and return them. A
   * record with no value, a tombstone, goes on unjudged.
   */
  static List<Refusal> judge(ProduceRequestData request, Predicate<TopicProduceData> gated, Gate gate) {
    List<Refusal> refusals = new ArrayList<>();
    for (Iterator<TopicProduceData> topics = request.topicData().iterator(); topics.hasNext(); ) {
      TopicProduceData topic = topics.next();
      if (!gated.test(topic)) {
        continue;
      }
      for (Iterator<PartitionProduceData> partitions = topic.partitionData().iterator();
          partitions.hasNext(); ) {
        PartitionProduceData partition = partitions.next();
        List<BatchIndexAndErrorMessage> errors = errors(partition, gate);
        if (!errors.isEmpty()) {
          refusals.add(new Refusal(topic.name(), topic.topicId(), partition.index(), errors));
          partitions.remove();
        }
      }
      if (topic.partitionData().isEmpty()) {
        topics.remove();
      }
    }
    return refusals;
  }

  /** A record error for each record of the partition that the gate blocks. */
  private static List<BatchIndexAndErrorMessage> errors(PartitionProduceData partition, Gate gate) {
    List<BatchIndexAndErrorMessage> errors = new ArrayList<>();
    if (!(partition.records() instanceof MemoryRecords records)) {
      return errors;
    }
    int index = 0;
    for (RecordBatch batch : records.batches()) {
      for (Record record : batch) {
        if (record.hasValue() && gate.judge(bytes(record.value())) == Verdict.BLOCK) {
          errors.add(
              new BatchIndexAndErrorMessage()
                  .setBatchIndex(index)
                  .setBatchIndexErrorMessage(gate.deadLetter()));
        }
        index++;
      }
    }
    return errors;
  }

  private static byte[] bytes(ByteBuffer value) {
    byte[] bytes = new byte[value.remaining()];
    value.duplicate().get(bytes);
    return bytes;
  }

  /** A response holding only {@code refusals}, for a request with nothing left to forward. */
  static ProduceResponseData response(List<Refusal> refusals) {
    ProduceResponseData response = new ProduceResponseData();
    merge(response, refusals);
    return response;
  }

  /** Add {@code refusals} to the broker's response to what was forwarded. */
  static void merge(ProduceResponseData response, List<Refusal> refusals) {
    for (Refusal refusal : refusals) {
      TopicProduceResponse topic = response.responses().find(refusal.topic(), refusal.topicId());
      if (topic == null) {
        topic = new TopicProduceResponse().setName(refusal.topic()).setTopicId(refusal.topicId());
        response.responses().add(topic);
      }
      topic
          .partitionResponses()
          .add(
              new PartitionProduceResponse()
                  .setIndex(refusal.partition())
                  .setErrorCode(Errors.INVALID_RECORD.code())
                  .setBaseOffset(-1)
                  .setLogAppendTimeMs(-1)
                  .setLogStartOffset(-1)
                  .setRecordErrors(refusal.errors())
                  .setErrorMessage(
                      refusal.errors().size()
                          + " record(s) in the batch break the data contract; each record error"
                          + " carries its dead letter"));
    }
  }
}
