# Self-hosted GitLab: first real test

**Status:** open. The code is generic: `api_base` builds `https://<host>/api/v4` for any configured GitLab host, and no host name is hardcoded. It has not run against a real self-hosted instance.

**Where.** The company laptop (see `windows-support.md`), or a self-hosted GitLab CE container on a Linux machine if Docker is available.

**Steps.**

1. Declare the host in the recipe (`[hosts."<name>"] type = "gitlab"`) and add a credential rule for it to `credentials.toml`.
2. Run the GitLab loop test (`ci-loop-test-gitlab` in the trial history) against the instance.
3. Check: the internal CA is trusted through the platform verifier, `HTTPS_PROXY` is honoured, the CI dispatch works, the merge request opens.

**Done when.** One merge request opens on the self-hosted instance and the CI result is reported.
