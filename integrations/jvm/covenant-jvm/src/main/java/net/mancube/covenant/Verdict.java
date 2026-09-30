package net.mancube.covenant;

/** What a gate decides about one record. */
public enum Verdict {
  /** The record keeps the contract, and goes on. */
  PASS,
  /** It breaks the contract under {@code on_violation: block}: it is withheld. */
  BLOCK,
  /** It breaks the contract under {@code on_violation: warn}: it goes on, and is reported. */
  WARN
}
