# Agent image for GitHub Copilot CLI. It holds the agent and git, nothing else fleetify needs.
# Pinned to the version the adapter was checked against (1.0.101, see DECISIONS.md).
# Install method: npm package @github/copilot, Node.js 22 or later
# (https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli).
#
#   docker build -t fleetify-agent-copilot docker/agents/copilot

FROM node:22-bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN npm install -g @github/copilot@1.0.101 \
    && npm cache clean --force
LABEL io.fleetify.agent="copilot"
