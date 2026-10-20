"""Lower-bound check for a uv project.

The minimum versions below must be declared as lower bounds in pyproject.toml and
must not resolve below them in uv.lock. Run with the system python3, which has
tomllib, so the check does not depend on the project's environment.
"""

import sys
import tomllib
from pathlib import Path

# Package name -> minimum version. Extend this table when a bound must be kept.
MINIMUMS = {"pygments": "2.20.0"}


def version_key(text):
    return tuple(int(part) for part in text.split("."))


def main(repo):
    root = Path(repo)
    pyproject = tomllib.loads((root / "pyproject.toml").read_text())
    lock = tomllib.loads((root / "uv.lock").read_text())
    declared = pyproject["project"]["dependencies"]
    resolved = {pkg["name"]: pkg["version"] for pkg in lock.get("package", [])}

    problems = []
    for name, minimum in MINIMUMS.items():
        bound = [dep for dep in declared if dep.replace(" ", "").startswith(name + ">=")]
        if not bound:
            problems.append(f"{name}: no lower bound '{name}>={minimum}' in pyproject.toml")
        elif version_key(bound[0].split(">=")[1].strip()) < version_key(minimum):
            problems.append(f"{name}: lower bound in pyproject.toml is below {minimum}")
        if name not in resolved:
            problems.append(f"{name}: not in uv.lock")
        elif version_key(resolved[name]) < version_key(minimum):
            problems.append(f"{name}: uv.lock resolves {resolved[name]}, below {minimum}")

    if problems:
        for problem in problems:
            print(f"E  lower bound not met: {problem}")
        return 1
    print("ok: lower bounds met: " + ", ".join(f"{n}>={m}" for n, m in MINIMUMS.items()))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
