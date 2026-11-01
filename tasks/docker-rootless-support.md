# Docker: rootful, rootless, or both

**Status:** decided (2026-11-01): rootless only. Agent containers are refused on a rootful daemon. See `DECISIONS.md`, "Rootless Docker". The investigation below is kept for the record.

**Current state (2026-11-01, this machine).**

- The daemon is the system service (`/var/run/docker.sock`, `root:docker`, Docker 29.8.1, overlayfs, systemd cgroups). The user `darius` is in the `docker` group, which is root-equivalent: any container can mount the host's files.
- Rootless prerequisites are present: `rootlesskit`, `slirp4netns`, `dockerd-rootless-setuptool.sh`, `/etc/subuid` and `/etc/subgid` entries for `darius` (100000:65536), unprivileged user namespaces enabled. Rootless Docker is not installed or running (no `docker` user unit, no `$XDG_RUNTIME_DIR/docker.sock`).
- `/etc/docker/daemon.json` sets only `insecure-registries`. No DNS setting exists in the daemon config or in the repo.

**Known differences for fleetify.**

1. **Security.** Rootless removes the root-equivalent socket from the user. For company use this matters most.
2. **Workspace writes.** In rootless mode, container root maps to the host user, who owns the workspace. The `DAC_OVERRIDE` workaround in `src/container.rs` should then be unnecessary. To verify, not assumed.
3. **Socket.** fleetify calls `docker` with no host option. Rootless needs `DOCKER_HOST` or a context pointing at the user socket. Decide where that is set and record the daemon mode in the run log.
4. **Networking.** Rootless uses slirp4netns, not `docker0`. The proxy's reach to the LAN model host (`192.168.178.151`) and the internal `--internal` network must be tested again. The `fleetify-proxy` config and the canaries assume a bridge.
5. **Build DNS.** Build containers failed to resolve `deb.debian.org` on the default network (intermittent). Runtime DNS on the default bridge is reliable (5 of 5). Rootless builds may behave differently.

**Options.**

- **Rootful only.** No change. Fleetify keeps the root-equivalent socket and the `DAC_OVERRIDE` workaround.
- **Rootless default, rootful supported.** fleetify detects the mode (`docker info` security options contain `rootless`), prefers the user socket when present, and records the mode. Canaries run in both modes.
- **Rootless only.** Strongest boundary. Rootful hosts are refused. Needs the rootless setup on the company machine first.

**Recommendation.** Rootless default, rootful supported. Canaries and the network tests run in both modes before either is documented as supported.

**Done when.** The decision is in `DECISIONS.md`; if rootless is supported, `tests/network.rs` and `tests/opencode_network.rs` pass under rootless with the same results, and the run log names the daemon mode.
