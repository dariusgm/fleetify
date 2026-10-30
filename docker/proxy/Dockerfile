# The egress proxy of an agent run (stage 3). It runs the configuration fleetify writes for
# that run, mounted at /etc/fleetify/squid.conf, and nothing else from the host.
# Build: docker build -t fleetify-proxy:1 docker/proxy
FROM debian:bookworm-slim

LABEL io.fleetify.role="egress-proxy"

RUN apt-get update \
 && apt-get install -y --no-install-recommends squid bash \
 && rm -rf /var/lib/apt/lists/*

USER proxy
ENTRYPOINT ["squid", "-N", "-f", "/etc/fleetify/squid.conf"]
