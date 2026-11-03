# The change: re-resolve the uv lock within the constraints in pyproject.toml.
# Runs with PowerShell 7 (pwsh); the argument is the repository path.
param([Parameter(Mandatory = $true)][string]$Repo)
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $Repo
uv lock --upgrade
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
