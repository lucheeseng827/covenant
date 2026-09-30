package net.mancube.covenant.kroxylicious;

import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.stream.Collectors;
import io.kroxylicious.proxy.filter.FilterContext;
import io.kroxylicious.proxy.filter.ProduceRequestFilter;
import io.kroxylicious.proxy.filter.ProduceResponseFilter;
import io.kroxylicious.proxy.filter.RequestFilterResult;
import io.kroxylicious.proxy.filter.ResponseFilterResult;
import io.kroxylicious.proxy.filter.metadata.TopicNameMapping;
import net.mancube.covenant.Gate;
import org.apache.kafka.common.Uuid;
import org.apache.kafka.common.message.ProduceRequestData;
import org.apache.kafka.common.message.ProduceRequestData.TopicProduceData;
import org.apache.kafka.common.message.ProduceResponseData;
import org.apache.kafka.common.message.RequestHeaderData;
import org.apache.kafka.common.message.ResponseHeaderData;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * One connection's gate. Records of the gated topics are judged before their produce request
 * reaches a broker; see {@link Judgement} for what a refusal looks like to the producer.
 */
final class CovenantGateFilter implements ProduceRequestFilter, ProduceResponseFilter {
  private static final Logger LOG = LoggerFactory.getLogger(CovenantGateFilter.class);

  private final Gate gate;
  private final Set<String> topics;
  /** Partitions refused from requests forwarded without them, until the broker answers. */
  private final Map<Integer, List<Judgement.Refusal>> refused = new HashMap<>();

  CovenantGateFilter(Gate gate, Set<String> topics) {
    this.gate = gate;
    this.topics = topics;
  }

  @Override
  public CompletionStage<RequestFilterResult> onProduceRequest(
      short apiVersion, RequestHeaderData header, ProduceRequestData request, FilterContext context) {
    // Recent clients name a topic by its id; the gate is configured by name.
    Set<Uuid> unnamed =
        request.topicData().stream()
            .filter(t -> t.name() == null || t.name().isEmpty())
            .map(TopicProduceData::topicId)
            .collect(Collectors.toSet());
    CompletionStage<Map<Uuid, String>> names =
        unnamed.isEmpty()
            ? CompletableFuture.completedFuture(Map.of())
            : context.topicNames(unnamed).thenApply(TopicNameMapping::topicNames);
    return names.thenCompose(byId -> judge(header, request, context, byId));
  }

  private CompletionStage<RequestFilterResult> judge(
      RequestHeaderData header, ProduceRequestData request, FilterContext context, Map<Uuid, String> byId) {
    List<Judgement.Refusal> refusals =
        Judgement.judge(
            request,
            topic -> {
              String name =
                  topic.name() == null || topic.name().isEmpty() ? byId.get(topic.topicId()) : topic.name();
              return name != null && topics.contains(name);
            },
            gate);
    if (refusals.isEmpty()) {
      return context.forwardRequest(header, request);
    }
    LOG.warn(
        "covenant: refused {} partition batch(es) of a produce request on {}",
        refusals.size(),
        context.channelDescriptor());
    if (request.acks() == 0) {
      // No answer is awaited, so there is no one to tell: the rest goes on.
      return request.topicData().isEmpty()
          ? context.requestFilterResultBuilder().drop().completed()
          : context.forwardRequest(header, request);
    }
    if (request.topicData().isEmpty()) {
      return context
          .requestFilterResultBuilder()
          .shortCircuitResponse(Judgement.response(refusals))
          .completed();
    }
    refused.put(header.correlationId(), refusals);
    return context.forwardRequest(header, request);
  }

  @Override
  public CompletionStage<ResponseFilterResult> onProduceResponse(
      short apiVersion, ResponseHeaderData header, ProduceResponseData response, FilterContext context) {
    List<Judgement.Refusal> refusals = refused.remove(header.correlationId());
    if (refusals != null) {
      Judgement.merge(response, refusals);
    }
    return context.forwardResponse(header, response);
  }
}
