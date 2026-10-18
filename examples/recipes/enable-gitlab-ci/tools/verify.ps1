# Fast verifier: the pipeline file must exist. Exit 1 when it does not.
param([Parameter(Mandatory = $true)][string]$Repo)
Set-Location -LiteralPath $Repo
if (-not (Test-Path -LiteralPath '.gitlab-ci.yml')) {
    Write-Output 'E  .gitlab-ci.yml is missing'
    exit 1
}
Write-Output 'ok: .gitlab-ci.yml present'
