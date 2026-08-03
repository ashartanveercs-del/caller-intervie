[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$binaryDirectory = Join-Path $projectRoot "desktop\src-tauri\binaries"
$smokeScript = Join-Path $PSScriptRoot "test-sidecar-package.ps1"
$wrongBinary = Join-Path $binaryDirectory "callerinterview-sidecar-wrong-triple.exe"
Remove-Item -LiteralPath $wrongBinary -Force -ErrorAction SilentlyContinue
$binaries = @(Get-ChildItem $binaryDirectory -Filter "callerinterview-sidecar-*" -File | Where-Object {
    $_.Name -notlike "*.provenance.json" -and
    $_.Name -notmatch "\.(staged|backup|restore-discard)-"
})

if ($binaries.Count -ne 1) {
    throw "regression fixture requires exactly one packaged sidecar"
}

$actualBinary = $binaries[0]
$wrongBinary = Join-Path $binaryDirectory "callerinterview-sidecar-wrong-triple$($actualBinary.Extension)"
$backupBinary = "$($actualBinary.FullName).regression-backup-$PID"
$targetTriple = $actualBinary.BaseName -replace '^callerinterview-sidecar-', ''
$previousTargetTriple = $env:SIDECAR_TARGET_TRIPLE

Move-Item -LiteralPath $actualBinary.FullName -Destination $backupBinary
Copy-Item -LiteralPath $backupBinary -Destination $wrongBinary
try {
    $env:SIDECAR_TARGET_TRIPLE = $targetTriple
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $failureOutput = & powershell -ExecutionPolicy Bypass -File $smokeScript 2>&1
        $smokeExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($smokeExitCode -eq 0) {
        throw "smoke accepted stale wrong-triple binary"
    }
    if (($failureOutput | Out-String) -notmatch "sidecar binary (ambiguity|mismatch)") {
        throw "smoke failed for an unexpected reason: $failureOutput"
    }
}
finally {
    $env:SIDECAR_TARGET_TRIPLE = $previousTargetTriple
    Remove-Item -LiteralPath $wrongBinary -Force -ErrorAction SilentlyContinue
    Move-Item -LiteralPath $backupBinary -Destination $actualBinary.FullName -ErrorAction SilentlyContinue
}
