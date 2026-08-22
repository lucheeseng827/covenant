# Security policy

## Supported versions

Covenant is pre-1.0. Security fixes land on the latest released version only.

## Reporting a vulnerability

Please report security issues privately through GitHub's
[private vulnerability reporting](https://github.com/lucheeseng827/covenant/security/advisories/new)
rather than opening a public issue.

Include the version (`covenant --version`), what you ran, what happened, and
what you expected. A contract file and a minimal record that reproduces the
issue are the most useful things you can attach.

Expect an acknowledgement within a week. Once a fix is available, the advisory
is published together with the release that carries it.

## Scope notes

Covenant is a local binary and a library. It reads contract files and data you
point it at, writes reports and dead-letter files, and makes no outbound network
calls — there is no telemetry, no update check, and no phone-home on any code
path. (The optional `covenant serve` binds a local listening socket; see below.)

Two areas are worth attention when reviewing:

- **`covenant serve`** (off by default, behind `--features serve`) binds a
  localhost HTTP console with no authentication. It is a development tool. Do
  not expose it to a network you do not trust.
- **Contract-supplied regular expressions.** `pattern` constraints are compiled
  from the contract. A contract you did not write is untrusted input, the same
  as data; review it before enforcing it.
