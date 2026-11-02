# Copilot in containers: a broker that keeps the token out of the agent

**Status:** open, idea. Follow-up of `tasks/copilot-scoped-token.md`.

**Known.** The Copilot token is in the container's environment, so the agent can read it. The token is narrow (Copilot Requests only), so the damage is limited to the Copilot quota. Still, the agent holds a credential.

**Idea.** A broker outside the container makes the Copilot calls, and the agent only talks to the broker. The token never enters the container.

**Open.**

1. Decide where the broker runs: a process of fleetify on the host, reached through a Unix socket that is mounted into the container.
2. Decide what the broker accepts: only Copilot's API endpoints, with the proxy's allowlist in front of it.
3. Check whether Copilot CLI can use a custom base URL or a proxy that adds the authorization header. If not, the broker has to be a local HTTP proxy that injects the header.
4. Test: the token is absent from the container's environment and files, Copilot still answers `pong`, and the broker refuses other hosts.

**Done when.** The token is not in any container, and `tests/copilot_network.rs` passes through the broker.
