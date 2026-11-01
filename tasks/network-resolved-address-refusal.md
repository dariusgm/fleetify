# Network: refuse names that resolve to private, loopback or gateway addresses

**Status:** open. Security gap, highest priority of the stage-3 network items.

**Known.** `DECISIONS.md` ("Stage-3 network design") says: a name that resolves to the Docker gateway, loopback, link-local or another private range the recipe did not list is refused. The proxy is on the default bridge, so it can reach the host's gateway and the LAN.

**Gap.** `squid_conf` in `src/egress.rs` has only `dstdomain` and `dst` allow rules for the listed targets, and `http_access deny all` at the end. A listed name that resolves to `172.17.0.1` (docker0), `127.0.0.1`, `169.254.0.0/16` or a LAN address is allowed. No test covers it.

**Open.**

1. Add Squid `dst` deny rules for loopback, link-local, RFC 1918 and the docker bridge range, placed before the allow rules. Listed IP entries are excepted, because a listed IP is an explicit decision. Verify that Squid's `dst` ACL matches the resolved server address on a `dstdomain` request, since the rule only works if it does.
2. Add the integration test: a listed name that points at a private address (a container on the bridge, or an `/etc/hosts` entry in the proxy) is refused.
3. Decide whether the proxy should also be kept off the host's gateway by a firewall rule. Record the choice in `DECISIONS.md`.

**Done when.** The test fails on the current configuration and passes after the change, and the result is in `DECISIONS.md`.
