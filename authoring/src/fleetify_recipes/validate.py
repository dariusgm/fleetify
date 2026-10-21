"""Checks a recipe against the schema that `fleetify schema` prints.

The schema comes from the binary, so the check follows the binary that will run the recipe.
Needs the optional `jsonschema` package (`pip install fleetify-recipes[validate]`).
"""

from __future__ import annotations

import json
import subprocess
import tomllib
from typing import Any

from .recipe import Recipe, check_version, to_text


def fleetify_schema(binary: str = "fleetify") -> dict[str, Any]:
    """The JSON Schema of recipe.toml as the given fleetify binary prints it."""
    result = subprocess.run([binary, "schema"], capture_output=True, text=True, check=True)
    return json.loads(result.stdout)


def problems(text: str, schema: dict[str, Any]) -> list[str]:
    """Every schema violation of one recipe.toml text, as readable messages."""
    try:
        from jsonschema import Draft202012Validator
    except ImportError as err:
        raise RuntimeError(
            "validation needs jsonschema: pip install fleetify-recipes[validate]"
        ) from err
    document = tomllib.loads(text)
    validator = Draft202012Validator(schema)
    return [
        f"{'.'.join(str(part) for part in error.absolute_path) or '<root>'}: {error.message}"
        for error in sorted(validator.iter_errors(document), key=lambda e: list(e.absolute_path))
    ]


def check(recipe: Recipe, schema: dict[str, Any]) -> list[str]:
    """Checks the version, then the text the recipe would be written as."""
    check_version(recipe)
    return problems(to_text(recipe), schema)
