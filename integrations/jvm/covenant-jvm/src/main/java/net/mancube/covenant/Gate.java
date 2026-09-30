package net.mancube.covenant;

import com.dylibso.chicory.compiler.MachineFactoryCompiler;
import com.dylibso.chicory.runtime.ExportFunction;
import com.dylibso.chicory.runtime.ImportValues;
import com.dylibso.chicory.runtime.Instance;
import com.dylibso.chicory.runtime.Machine;
import com.dylibso.chicory.runtime.Memory;
import com.dylibso.chicory.wasi.WasiOptions;
import com.dylibso.chicory.wasi.WasiPreview1;
import com.dylibso.chicory.wasm.Parser;
import com.dylibso.chicory.wasm.WasmModule;
import java.io.IOException;
import java.io.InputStream;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.function.Function;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * The Covenant gate, one record at a time: the engine's own WebAssembly build ({@code
 * covenant_embed.wasm}, bundled in this jar) run on Chicory, compiled to JVM bytecode, with no
 * native code. It judges records exactly as {@code covenant gate} does and writes the same dead
 * letters.
 *
 * <pre>{@code
 * try (Gate gate = Gate.open(Path.of("orders.yaml"), null, UniqueKeys.SKIP)) {
 *   if (gate.judge(json) != Verdict.PASS) {
 *     deadLetters.send(gate.deadLetter());
 *   }
 * }
 * }</pre>
 *
 * <p>A gate is not thread-safe: give each thread, or each stream, its own. Opening one costs a
 * WebAssembly instance; the module is parsed and compiled once per class loader.
 */
public final class Gate implements AutoCloseable {
  private static final String MODULE = "covenant_embed.wasm";

  /** The module, parsed and compiled on first use. */
  private static final class Module {
    static final WasmModule WASM;
    static final Function<Instance, Machine> MACHINE;

    static {
      try (InputStream in = Gate.class.getResourceAsStream(MODULE)) {
        if (in == null) {
          throw new IllegalStateException(MODULE + " is missing from the jar");
        }
        WASM = Parser.parse(in);
      } catch (IOException e) {
        throw new UncheckedIOException(e);
      }
      MACHINE = MachineFactoryCompiler.compile(WASM);
    }
  }

  private static final Pattern COUNT = Pattern.compile("\"(\\w+)\":(\\d+|true|false)");

  private final WasiPreview1 wasi;
  private final Memory memory;
  // The module's exports: the C ABI of embed/src/lib.rs.
  private final ExportFunction alloc;
  private final ExportFunction free;
  private final ExportFunction openGate;
  private final ExportFunction judgeRecord;
  private final ExportFunction deadLetterOf;
  private final ExportFunction statsOf;
  private final ExportFunction errorOf;
  /** A buffer in the module's memory, reused for every record that fits. */
  private int buffer;
  private int capacity;
  private boolean closed;

  private Gate() {
    wasi =
        WasiPreview1.builder()
            .withOptions(WasiOptions.builder().withStderr(System.err).build())
            .build();
    Instance instance =
        Instance.builder(Module.WASM)
            .withImportValues(ImportValues.builder().addFunction(wasi.toHostFunctions()).build())
            .withMachineFactory(Module.MACHINE)
            .build();
    memory = instance.memory();
    alloc = instance.export("covenant_alloc");
    free = instance.export("covenant_free");
    openGate = instance.export("covenant_open");
    judgeRecord = instance.export("covenant_judge");
    deadLetterOf = instance.export("covenant_dead_letter");
    statsOf = instance.export("covenant_stats");
    errorOf = instance.export("covenant_error");
  }

  /**
   * Open a gate on a contract ({@code covenant: 1} or ODCS v3): load, lint and compile it,
   * exactly as {@code covenant gate} does, refusing one with rules this runtime cannot enforce.
   *
   * @param contract the contract document
   * @param origin its name, for messages (a file name)
   * @param model the model to enforce, or {@code null} for the contract's only model
   * @param unique what to do with the model's {@code unique} fields
   * @throws CovenantException if the contract cannot be enforced
   */
  public static Gate open(String contract, String origin, String model, UniqueKeys unique) {
    Gate gate = new Gate();
    int[] text = gate.put(contract.getBytes(StandardCharsets.UTF_8));
    int[] name = gate.put(origin.getBytes(StandardCharsets.UTF_8));
    int[] which = gate.put((model == null ? "" : model).getBytes(StandardCharsets.UTF_8));
    long rc =
        gate.openGate.apply(
            text[0], text[1], name[0], name[1], which[0], which[1], abi(unique))[0];
    for (int[] b : new int[][] {text, name, which}) {
      gate.free.apply(b[0], Math.max(b[1], 1));
    }
    if (rc != 0) {
      String why = gate.read(gate.errorOf.apply()[0]);
      gate.close();
      throw new CovenantException(why);
    }
    return gate;
  }

  /** The module's code for a {@link UniqueKeys} choice. */
  private static int abi(UniqueKeys unique) {
    return switch (unique) {
      case REFUSE -> 0;
      case SKIP -> 1;
      case TRACK -> 2;
    };
  }

  /**
   * Open a gate on the contract in {@code contract}; see {@link #open(String, String, String,
   * UniqueKeys)}.
   */
  public static Gate open(Path contract, String model, UniqueKeys unique) throws IOException {
    return open(
        Files.readString(contract), contract.getFileName().toString(), model, unique);
  }

  /**
   * Judge one record: the bytes of one JSON object. A record that is not UTF-8, not JSON or not
   * an object breaks the contract too.
   */
  public Verdict judge(byte[] record) {
    ensureOpen();
    if (record.length > capacity) {
      if (capacity > 0) {
        free.apply(buffer, capacity);
      }
      capacity = Math.max(record.length, 4096);
      buffer = (int) alloc.apply(capacity)[0];
    }
    memory.write(buffer, record);
    int verdict = (int) judgeRecord.apply(buffer, record.length)[0];
    switch (verdict) {
      case 0:
        return Verdict.PASS;
      case 1:
        return Verdict.BLOCK;
      case 2:
        return Verdict.WARN;
      default:
        throw new CovenantException("the module answered " + verdict + " to a judgement");
    }
  }

  /** Judge one record given as text; see {@link #judge(byte[])}. */
  public Verdict judge(String record) {
    return judge(record.getBytes(StandardCharsets.UTF_8));
  }

  /**
   * The dead letter of the record just judged, one JSON object exactly as {@code covenant gate}
   * writes it to its DLQ; {@code null} when the record passed.
   */
  public String deadLetter() {
    ensureOpen();
    String letter = read(deadLetterOf.apply()[0]);
    return letter.isEmpty() ? null : letter;
  }

  /** The counts so far, and whether enforcement fails the stream. */
  public Stats stats() {
    ensureOpen();
    Matcher m = COUNT.matcher(read(statsOf.apply()[0]));
    long records = 0, passed = 0, blocked = 0, warned = 0;
    boolean failed = false;
    while (m.find()) {
      switch (m.group(1)) {
        case "records" -> records = Long.parseLong(m.group(2));
        case "passed" -> passed = Long.parseLong(m.group(2));
        case "blocked" -> blocked = Long.parseLong(m.group(2));
        case "warned" -> warned = Long.parseLong(m.group(2));
        case "failed" -> failed = Boolean.parseBoolean(m.group(2));
        default -> {}
      }
    }
    return new Stats(records, passed, blocked, warned, failed);
  }

  /** Whether enforcement fails the stream so far. */
  public boolean failed() {
    return stats().failed();
  }

  /** Release the gate. Its keys and counts go with it. */
  @Override
  public void close() {
    if (!closed) {
      closed = true;
      wasi.close();
    }
  }

  private void ensureOpen() {
    if (closed) {
      throw new IllegalStateException("the gate is closed");
    }
  }

  /** Copy {@code bytes} into the module: its pointer and length. */
  private int[] put(byte[] bytes) {
    int ptr = (int) alloc.apply(Math.max(bytes.length, 1))[0];
    memory.write(ptr, bytes);
    return new int[] {ptr, bytes.length};
  }

  /** Read an answer the module packed as pointer and length. */
  private String read(long packed) {
    int ptr = (int) (packed >>> 32);
    int len = (int) packed;
    return new String(memory.readBytes(ptr, len), StandardCharsets.UTF_8);
  }
}
