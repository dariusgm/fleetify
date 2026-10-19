Convert this project from Poetry to uv. Keep everything else as it is.

1. pyproject.toml: move the dependencies from [tool.poetry.dependencies] to [project] (PEP 621), as PEP 508 requirement strings with the same version constraints. Move name, version, description and authors into [project]. Keep the Python requirement as requires-python = "==3.15.*". Remove all [tool.poetry] tables. Add [tool.uv] with package = false, because this project is not an installable package.
   Declare these minimum versions as explicit lower bounds in the dependency list, for example "pygments>=2.20.0". Do not lower them, and do not drop them when you rewrite the dependencies:
   - pygments>=2.20.0
2. Run `uv lock` to create uv.lock. Do not write uv.lock by hand.
3. Dockerfile: replace the Poetry steps with uv. Install uv with pip, copy pyproject.toml and uv.lock, run `uv sync --frozen --no-install-project`, copy the rest of the project, and build with `uv run make html`. Keep the base image and the other steps.
4. Do not change the Makefile, the content or any other file.
5. Do not add suppression comments.
