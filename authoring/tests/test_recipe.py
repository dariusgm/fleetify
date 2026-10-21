import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
EXAMPLE = ROOT / "examples" / "recipes" / "enable-gitlab-ci"
sys.path.insert(0, str(ROOT / "authoring" / "examples"))

from enable_gitlab_ci import enable_gitlab_ci  # noqa: E402
from fleetify_recipes import (  # noqa: E402
    AppliesTo,
    Limits,
    Recipe,
    SchemaVersionError,
    check,
    fleetify_schema,
    to_text,
    write,
)
from fleetify_recipes.toml import _value  # noqa: E402


def fleetify_binary() -> str | None:
    """The fleetify binary: $FLEETIFY_BIN, else the repo's debug build, else PATH."""
    candidates = [os.environ.get("FLEETIFY_BIN"), str(ROOT / "target" / "debug" / "fleetify")]
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return candidate
    return shutil.which("fleetify")


needs_binary = pytest.mark.skipif(fleetify_binary() is None, reason="no fleetify binary built")


def test_the_example_reproduces_the_hand_written_recipe_byte_for_byte(tmp_path):
    out = tmp_path / "recipe.toml"
    write(out, enable_gitlab_ci())
    assert out.read_bytes() == (EXAMPLE / "recipe.toml").read_bytes()


def test_a_schema_version_other_than_one_is_refused(tmp_path):
    recipe = Recipe(name="x", branch="b", pr_title="t", schema=2)
    with pytest.raises(SchemaVersionError):
        write(tmp_path / "recipe.toml", recipe)
    assert not (tmp_path / "recipe.toml").exists()


def test_backslash_strings_are_written_as_literal_strings():
    assert _value(r"#\s*nosec") == "'#\\s*nosec'"
    assert _value('say "hi"') == '"say \\"hi\\""'
    assert _value(["a", "b"]) == '["a", "b"]'
    assert _value([]) == "[]"
    assert _value(True) == "true"


def test_unset_fields_are_left_out():
    text = to_text(Recipe(name="x", branch="b", pr_title="t", applies_to=AppliesTo(host_type=["github"])))
    assert text == 'schema = 1\nname = "x"\nbranch = "b"\npr_title = "t"\n\n[applies_to]\nhost_type = ["github"]\n'


@needs_binary
def test_the_generated_recipe_matches_the_binary_schema():
    schema = fleetify_schema(fleetify_binary())
    assert check(enable_gitlab_ci(), schema) == []


@needs_binary
def test_a_wrong_value_is_reported_against_the_binary_schema():
    schema = fleetify_schema(fleetify_binary())
    recipe = enable_gitlab_ci()
    recipe.limits = Limits(agent_timeout_min="soon", idle_warn_min=10)
    found = check(recipe, schema)
    assert any(message.startswith("limits.agent_timeout_min") for message in found), found


@needs_binary
def test_fleetify_validate_accepts_the_generated_recipe(tmp_path):
    recipe_dir = tmp_path / "enable-gitlab-ci"
    shutil.copytree(EXAMPLE, recipe_dir)
    write(recipe_dir / "recipe.toml", enable_gitlab_ci())
    result = subprocess.run(
        [fleetify_binary(), "validate", str(recipe_dir)], capture_output=True, text=True
    )
    assert result.returncode == 0, result.stderr
