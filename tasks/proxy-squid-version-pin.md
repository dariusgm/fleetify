# Proxy: pin the Squid version of the proxy image

**Status:** open, small.

**Gap.** `DECISIONS.md` says "Squid, pinned version" and "Squid 5.7 on debian bookworm". `docker/proxy/Dockerfile` runs `apt-get install squid` with no version, so the version changes with the mirror.

**Open.**

1. Pin the package: `squid=<version>` in the `apt-get install` line, with the version checked against `apt-cache policy squid` in the image. Or pin the base image by digest.
2. Record the version and the digest in `DECISIONS.md`.
3. Rebuild `fleetify-proxy:1` and run `tests/network.rs`.

**Done when.** The Dockerfile names the version, and the network tests pass on the rebuilt image.
