# Agent image for opencode. It holds the agent and git, nothing else fleetify needs.
# Pinned to the version the adapter was checked against; see DECISIONS.md.
# Install method: the opencode-ai npm package (https://opencode.ai/docs/).
#
#   docker build -t fleetify-agent-opencode docker/agents/opencode

FROM node:22-bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN npm install -g opencode-ai@1.19.3 \
    && npm cache clean --force
LABEL io.fleetify.agent="opencode"
