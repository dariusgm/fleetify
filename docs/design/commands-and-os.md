# Commands, shells and operating systems

> Design of fleetify. Moved from the former PLAN.md when it was dissolved (2026-10-23). Describes the target design; open work is in `tasks/`.

- Commands are argument lists (`argv`, or `posix`/`windows` variants). There is no implicit shell. Placeholders (`{repo}`, `{tools}`) are substituted per element, never into a concatenated string.
- If a shell is needed it is explicit in the argument list (`pwsh`, `bash`, `sh`, `cmd`). On Windows PowerShell is the primary case: prefer `-File` with arguments over `-Command` strings to avoid quoting problems. Execution policy follows the machine's setup; fleetify does not bypass it.
- Child output is read as UTF-8 with replacement for invalid bytes. Line endings of repository files are preserved: fleetify must not convert them, and the guards ignore pure line-ending noise.
- Timeouts terminate the whole process tree: Job Objects on Windows, process groups on POSIX (`command-group` or equivalent; verify it actually kills grandchildren of `pwsh`). This needs tests on both.
- Paths use `std::path`, never hard-coded separators.
- Every `git` call goes through the same runner (so it is logged, timed out and redacted like everything else).
- The binary's own test suite runs on Windows and Linux (CI matrix); release builds are produced for `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu` (and `aarch64-apple-darwin` if a Mac is around).

