# covenant — distroless image for the `covenant` enforcement runtime.
#
# The build is pure-Rust with zero C/C++ deps (arrow/parquet, serde, regex are all
# pure-Rust), so the binary links fully static against musl (Alpine's native target)
# and drops into `distroless/static` — no libc, no shell, no package manager, runs as
# nonroot. Multi-arch (linux/amd64 + linux/arm64) via `docker buildx`:
#
#   docker buildx build --platform linux/amd64,linux/arm64 -t mancube/covenant .
#
# The image is meant for pipeline use: mount the contract + data, or pipe a stream
# through `covenant gate` (stdin/stdout work under `docker run -i`).
#
#   docker run -i -v $PWD:/w mancube/covenant check /w/dump.parquet -c /w/orders.yaml
#   kcat -C -t raw | docker run -i -v $PWD:/w mancube/covenant gate -c /w/orders.yaml
#
# NOTE (monorepo): building from the module dir alone won't resolve the root workspace;
# build with the repo root as context until the OSS mirror (standalone repo) exists:
#   docker build -f ./Dockerfile .
# The OSS sync rewrites the monorepo-specific bits for the public mirror
# (package name `covenant` -> `covenant`, the rust_modules/... COPY paths,
# and the SOURCE_REPOSITORY default).

FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
# Alpine's host target is *-unknown-linux-musl → a fully static binary by default.
# `-p covenant` works in the monorepo; in the OSS mirror the package is `covenant`
# and the -p flag is unnecessary.
RUN cargo build --release --bin covenant && \
    strip target/release/covenant

FROM gcr.io/distroless/static-debian12:nonroot
# Defaults to the monorepo (where this file lives today); the OSS sync
# overrides it to the public mirror so the label always names a repository
# that actually contains the built commit.
ARG SOURCE_REPOSITORY=https://github.com/lucheeseng827/covenant
LABEL org.opencontainers.image.source="${SOURCE_REPOSITORY}" \
      org.opencontainers.image.description="Covenant — data-contract enforcement runtime (check/gate/diff at the producer boundary)" \
      org.opencontainers.image.licenses="Apache-2.0"
# Apache-2.0 requires license + attribution notices to travel with
# redistributed binaries.
COPY --from=build /src/./LICENSE /usr/share/doc/covenant/LICENSE
COPY --from=build /src/./NOTICE /usr/share/doc/covenant/NOTICE
COPY --from=build /src/target/release/covenant /usr/local/bin/covenant
ENTRYPOINT ["/usr/local/bin/covenant"]
CMD ["--help"]
