# Verifier: the lock must match pyproject.toml, and the tests must pass.
param([Parameter(Mandatory = $true)][string]$Repo)
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $Repo
uv lock --check
if ($LASTEXITCODE -ne 0) { Write-Output 'E  uv.lock does not match pyproject.toml'; exit 1 }
uv run pytest -q
if ($LASTEXITCODE -ne 0) { Write-Output 'E  tests failed'; exit 1 }
Write-Output 'ok: lock and tests'
