# Agent image for Claude Code with uv and Python 3.10, for Python recipes whose verifier runs
# inside the agent's container. It builds on the Claude Code image; Python 3.10 is installed at
# build time, so a run does not download it.
#
#   docker build -t fleetify-agent-claude-code-py310 docker/agents/claude-code-python

FROM fleetify-agent-claude-code
COPY --from=ghcr.io/astral-sh/uv:0.13.1 /uv /usr/local/bin/uv
ENV UV_PYTHON_INSTALL_DIR=/opt/python
RUN uv python install 3.10
LABEL io.fleetify.agent="claude-code"
