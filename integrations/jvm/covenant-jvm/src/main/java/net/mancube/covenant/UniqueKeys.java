package net.mancube.covenant;

/**
 * What a gate does with a model's {@code unique} fields. A gate holds the keys it has seen in
 * memory, so it can hold {@code unique} only across what it judges itself: one process, one run.
 */
public enum UniqueKeys {
  /** Refuse to open a gate on a model that declares {@code unique} fields. */
  REFUSE,
  /** Open it, and do not enforce them. */
  SKIP,
  /** Enforce them, exactly, across every record this gate judges. */
  TRACK
}
