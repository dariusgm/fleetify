# Agent image for Claude Code. It holds the agent and git, nothing else fleetify needs.
# Claude Code is pinned to the version fleetify's adapter was checked against; see
# DECISIONS.md. Install method: https://code.claude.com/docs/en/setup (npm package).
#
#   docker build -t fleetify-agent-claude-code docker/agents/claude-code

FROM node:22-bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN npm install -g @anthropic-ai/claude-code@2.1.312 \
    && npm cache clean --force
# The image is replaced by a new build, not updated in place.
ENV DISABLE_AUTOUPDATER=1
LABEL io.fleetify.agent="claude-code"
