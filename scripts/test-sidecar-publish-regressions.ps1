[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$binaryDirectory = Join-Path $projectRoot "desktop\src-tauri\binaries"
$binaries = @(Get-ChildItem $binaryDirectory -Filter "callerinterview-sidecar-*" -File | Where-Object {
    $_.Name -notlike "*.provenance.json" -and
    $_.Name -notmatch "\.(staged|backup|restore-discard)-"
})
if ($binaries.Count -ne 1) {
    throw "publish regression requires exactly one packaged sidecar"
}
$binary = $binaries[0].FullName
$targetTriple = $binaries[0].BaseName -replace '^callerinterview-sidecar-', ''
$provenance = "$binary.provenance.json"
$publisher = Join-Path $PSScriptRoot "publish-sidecar-artifact.ps1"
$smokeScript = Join-Path $PSScriptRoot "test-sidecar-package.ps1"

if (-not (Test-Path -LiteralPath $provenance -PathType Leaf)) {
    throw "publish regression requires a valid current sidecar and provenance"
}

foreach ($fault in @("copy", "replace")) {
    $binaryHash = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash
    $provenanceHash = (Get-FileHash -LiteralPath $provenance -Algorithm SHA256).Hash
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $failureOutput = & powershell -ExecutionPolicy Bypass -File $publisher -SourceBinary $binary -TargetTriple $targetTriple -FaultInjection $fault 2>&1
        $publishExitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($publishExitCode -eq 0) {
        throw "publisher accepted injected $fault failure"
    }
    if (($failureOutput | Out-String) -notmatch "fault injection") {
        throw "publisher failed for an unexpected reason: $failureOutput"
    }
    if ((Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash -cne $binaryHash) {
        throw "publisher $fault failure changed the prior binary"
    }
    if ((Get-FileHash -LiteralPath $provenance -Algorithm SHA256).Hash -cne $provenanceHash) {
        throw "publisher $fault failure changed the prior provenance"
    }
    & powershell -ExecutionPolicy Bypass -File $smokeScript -TargetTriple $targetTriple -StartupBudgetMs 120000
    if ($LASTEXITCODE -ne 0) {
        throw "prior sidecar was not smokeable after injected $fault failure"
    }
}
