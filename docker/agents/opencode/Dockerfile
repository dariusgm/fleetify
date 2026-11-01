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
# opencode installs its plugin SDK into its config directory on the first session, from the
# npm registry. Install it here, pinned to the same version, so a run needs no registry access
# (a network-restricted run would otherwise log a refused request on every start).
RUN mkdir -p /root/.config/opencode \
    && cd /root/.config/opencode \
    && printf '{\n  "dependencies": {\n    "@opencode-ai/plugin": "1.19.3"\n  }\n}\n' > package.json \
    && npm install --no-audit --no-fund \
    && npm cache clean --force
