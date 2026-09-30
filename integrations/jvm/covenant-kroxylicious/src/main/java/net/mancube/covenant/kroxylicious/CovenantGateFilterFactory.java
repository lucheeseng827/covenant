package net.mancube.covenant.kroxylicious;

import com.fasterxml.jackson.annotation.JsonCreator;
import com.fasterxml.jackson.annotation.JsonProperty;
import io.kroxylicious.proxy.filter.Filter;
import io.kroxylicious.proxy.filter.FilterFactory;
import io.kroxylicious.proxy.filter.FilterFactoryContext;
import io.kroxylicious.proxy.plugin.Plugin;
import io.kroxylicious.proxy.plugin.PluginConfigurationException;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.Locale;
import java.util.Set;
import net.mancube.covenant.CovenantException;
import net.mancube.covenant.Gate;
import net.mancube.covenant.UniqueKeys;

/**
 * The gate as a Kroxylicious filter. In the proxy's configuration:
 *
 * <pre>
 * filterDefinitions:
 *   - name: covenant
 *     type: CovenantGateFilterFactory
 *     config:
 *       contract: /etc/covenant/orders.yaml
 *       topics: [orders]
 *       unique: skip
 * defaultFilters:
 *   - covenant
 * </pre>
 *
 * <p>The contract is read, linted and compiled when the proxy starts, and one that cannot be
 * enforced exactly stops it from starting. Each client connection gets its own gate.
 */
@Plugin(configType = CovenantGateFilterFactory.Config.class)
public class CovenantGateFilterFactory
    implements FilterFactory<CovenantGateFilterFactory.Config, CovenantGateFilterFactory.Contract> {

  /**
   * The filter's configuration.
   *
   * @param contract path to the contract, {@code covenant: 1} or ODCS v3
   * @param model the model to enforce, when the contract has more than one
   * @param topics the topics whose records are judged
   * @param unique {@code refuse} (the default) a model that declares {@code unique} fields, or
   *     {@code skip} them: a proxy judges each connection apart, so it cannot hold them
   */
  public record Config(String contract, String model, List<String> topics, String unique) {
    @JsonCreator
    public Config(
        @JsonProperty(value = "contract", required = true) String contract,
        @JsonProperty("model") String model,
        @JsonProperty(value = "topics", required = true) List<String> topics,
        @JsonProperty("unique") String unique) {
      this.contract = contract;
      this.model = model;
      this.topics = topics;
      this.unique = unique;
    }
  }

  /** The contract as read and checked at start, for every connection's gate. */
  public record Contract(String text, String origin, String model, Set<String> topics, UniqueKeys unique) {}

  @Override
  public Contract initialize(FilterFactoryContext context, Config config) {
    if (config == null || config.contract() == null) {
      throw new PluginConfigurationException("covenant: set config.contract to the contract to enforce");
    }
    if (config.topics() == null || config.topics().isEmpty()) {
      throw new PluginConfigurationException("covenant: set config.topics to the topics to gate");
    }
    UniqueKeys unique;
    switch (config.unique() == null ? "refuse" : config.unique().toLowerCase(Locale.ROOT)) {
      case "refuse" -> unique = UniqueKeys.REFUSE;
      case "skip" -> unique = UniqueKeys.SKIP;
      default ->
          throw new PluginConfigurationException(
              "covenant: unique must be refuse or skip; a proxy judges each connection apart, so it"
                  + " cannot track unique keys across a topic");
    }
    Path path = Path.of(config.contract());
    Contract contract;
    try {
      contract =
          new Contract(
              Files.readString(path),
              path.getFileName().toString(),
              config.model(),
              Set.copyOf(config.topics()),
              unique);
    } catch (IOException e) {
      throw new PluginConfigurationException("covenant: cannot read " + path + ": " + e, e);
    }
    // Open a gate now, so a contract that cannot be enforced stops the proxy here.
    try (Gate gate = open(contract)) {
      return contract;
    } catch (CovenantException e) {
      throw new PluginConfigurationException("covenant: " + e.getMessage(), e);
    }
  }

  @Override
  public Filter createFilter(FilterFactoryContext context, Contract contract) {
    return new CovenantGateFilter(open(contract), contract.topics());
  }

  private static Gate open(Contract contract) {
    return Gate.open(contract.text(), contract.origin(), contract.model(), contract.unique());
  }
}
