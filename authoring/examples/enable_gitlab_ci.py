"""Writes the enable-gitlab-ci recipe. Its output equals examples/recipes/enable-gitlab-ci/recipe.toml byte for byte.

Usage: python examples/enable_gitlab_ci.py <output-path>
"""

import sys

from fleetify_recipes import (
    AppliesTo,
    Ci,
    Guards,
    Instruction,
    Limits,
    Literal,
    Recipe,
    VerifyTable,
    write,
)


def enable_gitlab_ci() -> Recipe:
    return Recipe(
        schema=1,
        name="enable-gitlab-ci",
        branch="chore/enable-gitlab-ci",
        pr_title="chore: enable GitLab CI pipeline",
        agent="claude-code",
        attachments=["reference/gitlab-ci.yaml"],
        tools="tools",
        applies_to=AppliesTo(host_type=["gitlab"], not_has_file=[".gitlab-ci.yml"]),
        instructions=[
            Instruction(file="prompts/enable-ci.md"),
            Instruction(
                text="The reference pipeline is in .agent/attachments/gitlab-ci.yaml. "
                "Adapt it to this repository and write .gitlab-ci.yml."
            ),
        ],
        verify=VerifyTable(
            posix=["bash", "{tools}/verify.sh", "{repo}"],
            windows=["pwsh", "-NoProfile", "-File", "{tools}/verify.ps1", "{repo}"],
            timeout_s=300,
            tail_lines=40,
            error_patterns=[Literal(r"^E\s"), Literal("error:")],
        ),
        guards=Guards(
            deny_added_patterns=["NOSONAR", Literal(r"#\s*nosec"), Literal(r"#\s*noqa")],
            max_files_changed=50,
            max_lines_changed=2000,
        ),
        ci=Ci(provider="auto", wait=False, timeout_min=30, merge="off"),
        limits=Limits(agent_timeout_min=60, idle_warn_min=10),
    )


if __name__ == "__main__":
    write(sys.argv[1], enable_gitlab_ci())
