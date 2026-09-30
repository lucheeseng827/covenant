# ODCS example contracts

Copied from the Open Data Contract Standard repository,
<https://github.com/bitol-io/open-data-contract-standard>, directory `docs/examples/`, at
commit `d3e1cb3` (2026-09-11). They are Apache-2.0 licensed, and each file keeps its
copyright header.

Two examples are not included: `authoritative-definitions/authoritative-definitions.odcs.yaml`,
and the minimal example of the top-level `price` block.

Six files are changed, and each says so in a comment under its copyright header: three catalog
URLs, a broker host, the `engine` of two custom rules and one `vendor` field now name
placeholders instead of products, and one section comment is reworded. None of this changes how
a contract maps: the same rules are enforced, and the same ones are reported.

`tests/odcs_test.rs` loads every file here: each must read as an ODCS contract, and every rule
Covenant cannot enforce must be reported with its path, never dropped. To refresh, copy the
directory again at a newer commit, leave out the two examples above, redo the six changes, and
update the commit above.
