"""Dataclasses that mirror recipe.toml (schema version 1).

Field order is the order in which keys are written, so it follows the hand-written files
in examples/recipes. An unset field (None, or an empty list) is left out of the file.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

from .toml import dumps

SCHEMA_VERSION = 1


class SchemaVersionError(ValueError):
    """The recipe asks for another schema version than this package writes."""


@dataclass
class AppliesTo:
    host_type: list[str] | None = None
    has_file: list[str] | None = None
    has_file_any: list[str] | None = None
    not_has_file: list[str] | None = None
    path_glob: list[str] | None = None


@dataclass
class Instruction:
    file: str | None = None
    text: str | None = None


@dataclass
class CommandTable:
    argv: list[str] | None = None
    posix: list[str] | None = None
    windows: list[str] | None = None
    timeout_s: int | None = None


@dataclass
class VerifyTable:
    posix: list[str] | None = None
    windows: list[str] | None = None
    timeout_s: int | None = None
    tail_lines: int | None = None
    error_patterns: list[str] | None = None
    argv: list[str] | None = None
    baseline: bool | None = None


@dataclass
class Needs:
    commands: list[str] | None = None
    web: bool | None = None
    extra_directories: list[str] | None = None
    unrestricted: bool | None = None


@dataclass
class HostEntry:
    """`[hosts."<name>"]`: the host type of a host beyond github.com and gitlab.com."""

    type: str


@dataclass
class AgentSettings:
    """`[agents.<name>]`: the model for an agent the recipe does not name with `model`."""

    model: str | None = None


@dataclass
class RateLimit:
    max_wait_min: int | None = None


@dataclass
class Policy:
    """Loosening `require_checked_agent` needs `[permissions] unchecked_agents` in
    credentials.toml."""

    require_checked_agent: bool | None = None


@dataclass
class Delivery:
    open_mr: bool | None = None
    delete_branch: bool | None = None


@dataclass
class Guards:
    deny_added_patterns: list[str] | None = None
    deny_paths: list[str] | None = None
    allow_paths: list[str] | None = None
    max_files_changed: int | None = None
    max_lines_changed: int | None = None
    forbid_binary_files: bool | None = None
    forbid_test_deletion: bool | None = None


@dataclass
class Ci:
    provider: str | None = None
    wait: bool | None = None
    timeout_min: int | None = None
    merge: str | None = None
    validate: bool | None = None
    dispatch_workflow: str | None = None
    dispatch_after_s: int | None = None
    log_lines: int | None = None
    log_chars: int | None = None


@dataclass
class Limits:
    agent_timeout_min: int | None = None
    idle_warn_min: int | None = None


@dataclass(kw_only=True)
class Recipe:
    schema: int = SCHEMA_VERSION
    name: str
    branch: str
    branch_prefix: str | None = None
    pr_title: str
    commit_message: str | None = None
    agent: str | None = None
    model: str | None = None
    agents: dict[str, AgentSettings] | None = None
    attachments: list[str] = field(default_factory=list)
    tools: str | None = None
    applies_to: AppliesTo | None = None
    hosts: dict[str, HostEntry] | None = None
    instructions: list[Instruction] = field(default_factory=list)
    prepare: CommandTable | None = None
    verify: VerifyTable | None = None
    needs: Needs | None = None
    delivery: Delivery | None = None
    guards: Guards | None = None
    ci: Ci | None = None
    limits: Limits | None = None
    rate_limit: RateLimit | None = None
    policy: Policy | None = None


def check_version(recipe: Recipe) -> None:
    """Refuses a recipe whose schema version this package does not write."""
    if recipe.schema != SCHEMA_VERSION:
        raise SchemaVersionError(
            f"recipe schema {recipe.schema} is not supported; "
            f"this package writes schema {SCHEMA_VERSION}"
        )


def to_text(recipe: Recipe) -> str:
    """The recipe.toml text, after the version check."""
    check_version(recipe)
    return dumps(recipe)


def write(path: str | Path, recipe: Recipe) -> str:
    """Writes recipe.toml to `path` as UTF-8 with LF line endings and returns the text."""
    text = to_text(recipe)
    Path(path).write_bytes(text.encode("utf-8"))
    return text
