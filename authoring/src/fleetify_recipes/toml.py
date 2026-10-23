"""A small TOML writer for recipe.toml.

It writes the subset recipes use: strings, booleans, integers, arrays of those, tables,
arrays of tables, and tables keyed by name (``[hosts."name"]``). Output is deterministic:
fields in declaration order, one blank line before each table header, strings with a
backslash in single quotes (TOML literal strings, so a regex such as ``\\s`` needs no
escaping).
"""

from __future__ import annotations

import dataclasses
from typing import Any


class Literal(str):
    """A string written as a TOML literal string, in single quotes. Use it where a hand-written
    recipe quotes a value that way, for example a regex next to plain strings."""


def _value(value: Any) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, Literal):
        if "'" in value or "\n" in value:
            raise ValueError(f"cannot write {value!r} as a TOML literal string")
        return f"'{value}'"
    if isinstance(value, str):
        if "\\" in value:
            if "'" in value or "\n" in value:
                raise ValueError(f"cannot write {value!r} as a TOML literal string")
            return f"'{value}'"
        escaped = value.replace('"', '\\"')
        return f'"{escaped}"'
    if isinstance(value, (list, tuple)):
        return "[" + ", ".join(_value(item) for item in value) + "]"
    raise TypeError(f"no TOML form for {type(value).__name__}")


def _is_table(value: Any) -> bool:
    return dataclasses.is_dataclass(value) and not isinstance(value, type)


def _is_table_list(value: Any) -> bool:
    return isinstance(value, (list, tuple)) and bool(value) and all(_is_table(v) for v in value)


def _is_keyed_tables(value: Any) -> bool:
    return isinstance(value, dict) and bool(value) and all(_is_table(v) for v in value.values())


def _key(name: str) -> str:
    """A table key as TOML writes it: bare when it can be, quoted otherwise."""
    if name and all(c.isascii() and (c.isalnum() or c in "_-") for c in name):
        return name
    return '"' + name.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _fields(obj: Any) -> list[tuple[str, Any]]:
    """Declared fields, without the ones that are unset (None) or empty lists."""
    out = []
    for field in dataclasses.fields(obj):
        value = getattr(obj, field.name)
        if value is None:
            continue
        if isinstance(value, (list, tuple, dict)) and not value:
            continue
        out.append((field.name, value))
    return out


def _table_lines(name: str, obj: Any, array: bool) -> list[str]:
    header = f"[[{name}]]" if array else f"[{name}]"
    lines = ["", header]
    lines += _body(obj)
    return lines


def _body(obj: Any) -> list[str]:
    scalars = []
    tables = []
    for key, value in _fields(obj):
        if _is_table(value):
            tables.append((key, value, False))
        elif _is_table_list(value):
            tables.append((key, value, True))
        elif _is_keyed_tables(value):
            for name, item in value.items():
                tables.append((f"{key}.{_key(name)}", item, False))
        else:
            scalars.append(f"{key} = {_value(value)}")
    lines = scalars
    for key, value, array in tables:
        if array:
            for item in value:
                lines += _table_lines(key, item, True)
        else:
            lines += _table_lines(key, value, False)
    return lines


def dumps(obj: Any) -> str:
    """The TOML text of a dataclass tree. The top level has no header of its own."""
    lines = _body(obj)
    # The first table header in the file follows the top-level keys with a blank line; the
    # line that _table_lines adds for each header is already in place.
    return "\n".join(lines) + "\n"
