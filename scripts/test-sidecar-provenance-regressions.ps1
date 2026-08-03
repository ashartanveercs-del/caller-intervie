[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$smokeScript = Join-Path $PSScriptRoot "test-sidecar-package.ps1"
$packagingInput = Join-Path $PSScriptRoot "build-sidecar.ps1"
$backup = "$packagingInput.provenance-regression-backup-$PID"
$binaryDirectory = Join-Path $projectRoot "desktop\src-tauri\binaries"
$binaries = @(Get-ChildItem $binaryDirectory -Filter "callerinterview-sidecar-*" -File | Where-Object {
    $_.Name -notlike "*.provenance.json" -and
    $_.Name -notmatch "\.(staged|backup|restore-discard)-"
})
if ($binaries.Count -ne 1) {
    throw "provenance regression requires exactly one packaged sidecar"
}
$targetTriple = $binaries[0].BaseName -replace '^callerinterview-sidecar-', ''

Copy-Item -LiteralPath $packagingInput -Destination $backup
try {
    Add-Content -LiteralPath $packagingInput -Value "# provenance regression $PID"
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $failureOutput = & powershell -ExecutionPolicy Bypass -File $smokeScript -TargetTriple $targetTriple -StartupBudgetMs 120000 2>&1
        $smokeExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($smokeExitCode -eq 0) {
        throw "smoke accepted an old exact-name binary after a packaging input changed"
    }
    if (($failureOutput | Out-String) -notmatch "build receipt packaging inputs do not match") {
        throw "smoke failed for an unexpected reason: $failureOutput"
    }
}
finally {
    Move-Item -LiteralPath $backup -Destination $packagingInput -Force -ErrorAction SilentlyContinue
}
