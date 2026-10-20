Enable a GitLab CI pipeline for this repository.

- Read the existing build and test setup (package files, lock files, test
  directories) and find the commands that build and test this project.
- Adapt the reference pipeline in the attachments to those commands.
- Write `.gitlab-ci.yml` in the repository root. Change nothing else.
- Do not add suppression comments, skip markers or disabled checks.
- If you cannot determine the build or test commands, write `.agent/blocked.md`
  explaining what is missing instead of guessing.
