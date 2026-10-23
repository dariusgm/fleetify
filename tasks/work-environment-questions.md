# Open questions for the company environment

These questions come from PLAN.md and decide the work rollout. Answer them before the company trial.

1. **Tokens and roles.** Which tokens exist on the company GitLab (fine-grained per repository, per group, per user), who manages them, and which role may push branches and create merge requests.
2. **Protected branches.** Are `chore/*` branches protected?
3. **Proxy.** Is there a corporate proxy? The HTTP client must honour `HTTPS_PROXY`.
4. **GitHub at work.** Is it github.com or GitHub Enterprise Server?
5. **Agents at work.** How does Copilot CLI authenticate, and may company repositories be processed only with Copilot? A private Claude Code on company code is probably not allowed; compliance must clear it.
6. **CI at work.** Jenkins is the CI today, a GitLab CI migration is planned. Are pipelines enabled on the target projects, and do the pipeline rules cover `chore/*` branches and merge request pipelines?
7. **`pwsh` and Bash.** Is `pwsh` available where company runs happen, and which shell do verifier scripts expect?
8. **Judge provider.** Which provider and model are approved for diffs (only relevant with M16).
9. **Binary distribution.** Can an unsigned binary run on the work Windows machine, or does it need signing or an allowlisting ticket?
10. **Name and publication.** PyPI, npm and trademark checks are not done. Check before publishing.
