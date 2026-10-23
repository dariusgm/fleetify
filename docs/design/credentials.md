# Configuration and credentials

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

Credentials live in `~/.config/fleetify/credentials.toml`, owner-only, next to the configuration file. Behaviour settings are moving into the recipe (see tasks/remove-config-layer.md); until that is done, `config.toml` next to it still holds the hosts and permissions.

```toml
# ~/.config/fleetify/credentials.toml  (secrets, owner-only permissions)
# First matching rule wins. Put more specific rules first.
[[credential]]
match = "gitlab.example.com/team-a/project-x"   # single repo (fine-grained token)
token = "REPLACE_ME"

[[credential]]
match = "gitlab.example.com/team-a/*"           # e.g. a group token
token = "REPLACE_ME"

[[credential]]
match = "gitlab.example.com/*"                  # fallback, e.g. a user token
token = "REPLACE_ME"

[[credential]]
match = "github.com/myname/*"
token = "REPLACE_ME"
```

Rules:

- `match` is a glob on `<host>/<path>`. Order decides, not pattern length.
- No matching rule: the repo fails with a clear message (which repo, which rules were checked) and never prints a token value.
- Host tokens exist only inside the orchestrator process. They are **never put into environment variables**, so no subprocess can inherit them and no allow/deny list of variables has to be kept perfect. The agent process gets a minimal environment (allowlist) containing only what its adapter declares.
- Push and fetch over HTTPS: the token reaches only the short-lived `git` subprocess started by the orchestrator, via a `GIT_ASKPASS` helper (the fleetify binary itself, invoked with a hidden subcommand and a one-time pipe or file descriptor, never the token on the command line). Tokens never appear in URLs, command lines, logs or `.git/config`. SSH uses the user's key.
- A redactor masks every known token value in all logs, events, terminal output and the report.
- Startup checks: on POSIX the file must not be readable by group or others; on Windows the tool warns if it can detect broader access and the README documents restricting the ACL to the current user.
- `--dry-run` shows which rule matches each repo and makes one read call per repo (status only: ok / 401 / 403 / 404, never the value). Write access shows up only at push time, unless the host API reports the token's access level for the project (check the API docs). Upcoming token expiry is warned about when the host exposes it.
- **Agent logins are not fleetify's business.** fleetify stores no agent tokens. An agent authenticates through its own configuration (config directory, keychain, its own environment variables); the adapter's allowlist names exactly which directories and variables are passed through. Copilot CLI must never receive a host token used for pushing.

**Honest limitation:** any process running as the same OS user, including the agent, can read `credentials.toml`. The file protects against accidental inheritance (the problem with environment variables), not against a hostile or prompt-injected agent. What reduces the risk: least-privilege tokens (per-repo or group tokens, short expiry), the permissions fleetify passes to the agent (workspace only), and in stage 2 running agents in a container without credentials. Stage 2 can also read tokens from the OS credential store instead of a file.

**TLS / internal CA:** the internal CA is installed in the operating system. The HTTP client uses the OS certificate store (`reqwest` with `rustls` + `rustls-native-certs`, or `native-tls`; decide in M5 and note the reason). A startup self-test makes one TLS connection per configured host and aborts with a clear message on certificate errors. `git` is a separate case: depending on the installation, Git for Windows uses OpenSSL with its own CA file or the Windows certificate store. Verify clone and push during testing.

