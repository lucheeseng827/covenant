package net.mancube.covenant;

/**
 * A gate that cannot open (a contract that does not parse, lint or compile, or one with rules
 * this runtime cannot enforce exactly) or a module that cannot run. Never a verdict: a record
 * that breaks the contract is {@link Verdict#BLOCK} or {@link Verdict#WARN}.
 */
public class CovenantException extends RuntimeException {
  private static final long serialVersionUID = 1L;

  public CovenantException(String message) {
    super(message);
  }

  public CovenantException(String message, Throwable cause) {
    super(message, cause);
  }
}
