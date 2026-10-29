# The fleetify orchestrator as an image. Build stage: the pure Rust binary, no native
# libraries. Runtime stage: git, the binary and the docker client, which the orchestrator
# uses to start agent containers (see README, "Containers").
#
#   docker build -t fleetify .

FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM docker:cli AS docker-cli

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=docker-cli /usr/local/bin/docker /usr/local/bin/docker
COPY --from=build /src/target/release/fleetify /usr/local/bin/fleetify
ENTRYPOINT ["fleetify"]
