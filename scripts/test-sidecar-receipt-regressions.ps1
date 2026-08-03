[CmdletBinding()]
param(
    [switch]$RequireBuildReceipt
)

$ErrorActionPreference = "Stop"

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$binaryDirectory = Join-Path $projectRoot "desktop\src-tauri\binaries"
$binaries = @(Get-ChildItem $binaryDirectory -Filter "callerinterview-sidecar-*" -File | Where-Object {
    $_.Name -notlike "*.provenance.json" -and
    $_.Name -notmatch "\.(staged|backup|restore-discard)-"
})
if ($binaries.Count -ne 1) {
    throw "receipt regression requires exactly one packaged sidecar"
}
$publisher = Join-Path $PSScriptRoot "publish-sidecar-artifact.ps1"
$targetTriple = $binaries[0].BaseName -replace '^callerinterview-sidecar-', ''
$previousErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
try {
    $failureOutput = & powershell -ExecutionPolicy Bypass -File $publisher -SourceBinary $binaries[0].FullName -TargetTriple $targetTriple 2>&1
    $publishExitCode = $LASTEXITCODE
}
finally {
    $ErrorActionPreference = $previousErrorActionPreference
}
if ($publishExitCode -eq 0) {
    throw "publisher minted provenance for an arbitrary binary without a build receipt"
}
if (($failureOutput | Out-String) -notmatch "build receipt") {
    throw "publisher failed for an unexpected reason: $failureOutput"
}

if ($RequireBuildReceipt) {
    $distBinary = Join-Path $projectRoot "sidecar\build\dist\callerinterview-sidecar$($binaries[0].Extension)"
    $distReceipt = "$distBinary.receipt.json"
    if (-not (Test-Path -LiteralPath $distBinary -PathType Leaf) -or -not (Test-Path -LiteralPath $distReceipt -PathType Leaf)) {
        throw "receipt regression requires the current PyInstaller dist binary and receipt"
    }
    $packagingInput = Join-Path $PSScriptRoot "build-sidecar.ps1"
    $backup = "$packagingInput.receipt-regression-backup-$PID"
    Copy-Item -LiteralPath $packagingInput -Destination $backup
    try {
        Add-Content -LiteralPath $packagingInput -Value "# receipt regression $PID"
        $previousErrorActionPreference = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        try {
            $failureOutput = & powershell -ExecutionPolicy Bypass -File $publisher -SourceBinary $distBinary -ReceiptPath $distReceipt -TargetTriple $targetTriple 2>&1
            $publishExitCode = $LASTEXITCODE
        }
        finally {
            $ErrorActionPreference = $previousErrorActionPreference
        }
        if ($publishExitCode -eq 0) {
            throw "publisher accepted an old binary with an old build receipt after input changed"
        }
        if (($failureOutput | Out-String) -notmatch "build receipt packaging inputs do not match") {
            throw "publisher failed for an unexpected reason: $failureOutput"
        }
    }
    finally {
        Move-Item -LiteralPath $backup -Destination $packagingInput -Force -ErrorAction SilentlyContinue
    }
}
