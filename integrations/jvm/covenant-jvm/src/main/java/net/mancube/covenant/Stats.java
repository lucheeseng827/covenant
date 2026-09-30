package net.mancube.covenant;

/**
 * A gate's counts so far.
 *
 * @param records records judged
 * @param passed records that went on, warned ones included
 * @param blocked records withheld
 * @param warned records that broke the contract under {@code on_violation: warn} and went on
 * @param failed whether enforcement fails the stream: under {@code block}, more violations than
 *     the contract's {@code max_violations}
 */
public record Stats(long records, long passed, long blocked, long warned, boolean failed) {}
