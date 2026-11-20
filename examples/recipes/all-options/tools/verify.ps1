# Verifier for windows: the change must not add whitespace errors. Exit 1 when it does.
param([Parameter(Mandatory = $true)][string]$Repo)
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $Repo
git diff --check
if ($LASTEXITCODE -ne 0) { Write-Output 'E  whitespace errors in the change'; exit 1 }
Write-Output 'ok: no whitespace errors'
