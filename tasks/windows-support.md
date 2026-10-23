# Windows support

**Status:** open. Needs a Windows machine: the company laptop or a Windows VM. Nothing here can be done on the Linux machine.

**Scope.**

- The manual check of the live view in Windows Terminal (M13).
- The Windows checks of each stage-1 milestone, as the acceptance rule in the working agreement requires: `git clone --local` with hard links on NTFS (open question 9), long paths and the 260-character limit, `pwsh` verifier scripts, termination of process trees.
- The Windows CI matrix again, once the Windows tests are green.
- Windows system locations for configuration and data (this follows the config-layer decision).
- The self-hosted GitLab test (M15b) on the company laptop, including the internal CA through the Windows certificate store and `HTTPS_PROXY`.

**Done when.** The checks pass on the company laptop, and the results are in DECISIONS.md.
