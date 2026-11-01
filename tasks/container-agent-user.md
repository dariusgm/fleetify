# Containers: run the agent as the host user, not as root with DAC_OVERRIDE

**Status:** closed by rootless Docker (2026-11-01, `DECISIONS.md`). Under rootless, container root is the host user, so the workspace is writable without `DAC_OVERRIDE` and files are owned by the user. The `DAC_OVERRIDE` line was removed.

The original text below is kept for the record.

**Known.** Agent containers run as root (no `USER` in `docker/agents/*`) with `--cap-drop ALL`. A root process without `DAC_OVERRIDE` cannot write to the workspace, which the host user owns: every write fails with "Permission denied". The fix in place adds `--cap-add DAC_OVERRIDE` in `run_argv` (`src/container.rs`). It lets container root bypass file permissions inside the container. The mounts still limit what it reaches, and nothing extra reaches the host. Recorded in `DECISIONS.md` ("Opencode through the proxy").

**Why this is not the end state.** Files the agent creates in the workspace are owned by root on the host. Running as the host uid would keep them owned by the user.

**Open.**

1. Run the container as the workspace owner: `--user <uid>:<gid>`, read from the workspace directory's metadata (no new dependency).
2. The uid has no passwd entry, so `HOME` must point at a writable place (tmpfs). opencode keeps its data in `HOME`. Claude Code and Copilot keep their login there too, so check each agent's login mount and config path.
3. Drop `DAC_OVERRIDE` once the uid change is in, and keep the canaries: the write test and the opencode container test must still pass.

**Done when.** The agents write files owned by the host user, `DAC_OVERRIDE` is gone from `run_argv`, and the write and opencode container tests pass for every agent image.
