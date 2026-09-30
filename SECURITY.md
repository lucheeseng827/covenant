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

## Disclosure history

Every advisory is listed here, newest first: when it was published, the versions it affects,
the release that fixed it, and who reported it (with their permission).

No advisories have been published.

## Releases

The container image is built and pushed by `.github/workflows/release.yml` from a version
tag, not on a maintainer's machine. From the first release after 0.1.0, every image it pushes
is signed and carries build provenance, and there is no signing key for anyone to hold or leak:

- **The signature is keyless.** Sigstore issues the release job a short-lived certificate for
  its GitHub identity (this repository, that workflow file, that tag) and records the
  signature in its public transparency log.
- **The provenance** (SLSA) records which workflow built the image, and from which commit. It
  is stored in this repository's attestations.

The release job runs these same checks before it reports success. To run them yourself on the
version you pull:

```bash
V=x.y.z   # the release you run
cosign verify "docker.io/mancube/covenant:$V" \
  --certificate-identity "https://github.com/lucheeseng827/covenant/.github/workflows/release.yml@refs/tags/v$V" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
gh attestation verify "oci://docker.io/mancube/covenant:$V" --repo lucheeseng827/covenant \
  --signer-workflow lucheeseng827/covenant/.github/workflows/release.yml --source-ref "refs/tags/v$V"
```

The first command proves the image was signed by the release workflow running on that tag; the
second, that the same workflow built it from that tag. Every image on Docker Hub is published by
a run on its release tag (a publish by hand has to be started on the tag too), so both commands
hold for every release. `0.1.0` predates them and is unsigned: `cosign verify` reports "no
signatures found" for it.

## Scope notes

Covenant is a local binary and a library. It reads contract files and data you
point it at, writes reports and dead-letter files, and makes no network call you
did not ask for — there is no telemetry, no update check, and no phone-home on any
code path. The only outbound connections are the ones a flag names: the Kafka
gate's brokers (`gate --brokers`, behind the `kafka` feature), and the URL a run's
report or lineage is sent to (`--report-to`, `covenant push`, `--openlineage`).
(The optional `covenant serve` binds a local listening socket; see below.)

The environment variables it reads are a CI provider's own (`GITHUB_*`,
`GITLAB_CI`, `CI_*`, `CI`), and only to name the run in a report you asked for
(`--report-json` or `--report-to` on `check`, `gate` or `diff`, or the Python
`Report`'s `report_document` and `write_report`); and, each only when its feature
is used, `COVENANT_SAMPLE_KEY` (hashed report samples), `COVENANT_TOKEN`
(sending a report), `OPENLINEAGE_API_KEY` (`--openlineage`) and the standard
proxy variables (`HTTPS_PROXY`, `ALL_PROXY`, `NO_PROXY`, …) for a URL off this
machine. That key and those tokens are read from the environment only, never
from the command line, and never printed.

Three areas are worth attention when reviewing:

- **`covenant serve`** (off by default, behind `--features serve`) binds a
  localhost HTTP console with no authentication. It is a development tool. Do
  not expose it to a network you do not trust.
- **Contract-supplied regular expressions.** `pattern` constraints are compiled
  from the contract. A contract you did not write is untrusted input, the same
  as data; review it before enforcing it.
- **Sending a report** (`--report-to`, `covenant push`, `--openlineage`; the
  `send` feature, on by default). The URL is `https`, or plain `http` to this
  machine only, and may not carry credentials; the server's certificate is
  checked against the Mozilla root certificates compiled into the binary, not
  the system's trust store, so an endpoint behind a private CA is refused.
  Redirects are not followed. A report carries no data values: its samples
  carry a value's type and length and, under `--report-samples hashed`, an
  HMAC of it under `COVENANT_SAMPLE_KEY`, whose holder can test a guessed value
  against the hash; keep that key as you would a password
  ([`docs/REPORT.md`](docs/REPORT.md)).
