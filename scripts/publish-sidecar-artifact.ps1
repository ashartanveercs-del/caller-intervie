[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SourceBinary,
    [Parameter(Mandatory = $true)]
    [string]$TargetTriple,
    [string]$ReceiptPath,
    [string]$PythonPath = $env:SIDECAR_PYTHON,
    [string]$FaultInjection = $env:SIDECAR_TEST_PUBLISH_FAILURE
)

$ErrorActionPreference = "Stop"

function Resolve-Python([string]$configuredPath, [string]$projectRoot) {
    if ($configuredPath) {
        if (-not (Test-Path -LiteralPath $configuredPath -PathType Leaf)) {
            throw "PYTHON override does not exist: $configuredPath"
        }
        return (Resolve-Path -LiteralPath $configuredPath).Path
    }
    foreach ($candidate in @(
        (Join-Path $projectRoot ".venv\Scripts\python.exe"),
        (Join-Path $projectRoot ".venv/bin/python")
    )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
    foreach ($commandName in @("python3", "python")) {
        $command = Get-Command $commandName -CommandType Application -ErrorAction SilentlyContinue
        if ($command) {
            return $command.Source
        }
    }
    throw "PYTHON was not found. Set SIDECAR_PYTHON or pass -PythonPath."
}

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$python = Resolve-Python $PythonPath $projectRoot
$smokeScript = Join-Path $PSScriptRoot "test-sidecar-package.ps1"
$provenanceHelper = Join-Path $projectRoot "sidecar\package_provenance.py"
$transactionModule = Join-Path $PSScriptRoot "sidecar-artifact-transaction.psm1"
if (-not (Test-Path -LiteralPath $SourceBinary -PathType Leaf)) {
    throw "sidecar source binary missing: $SourceBinary"
}
if (-not $ReceiptPath -or -not (Test-Path -LiteralPath $ReceiptPath -PathType Leaf)) {
    throw "sidecar build receipt is required: $ReceiptPath"
}
if ($FaultInjection -and $FaultInjection -notin @("copy", "before-first-swap", "after-both-swap")) {
    throw "unknown sidecar publish fault injection: $FaultInjection"
}
& $python $provenanceHelper validate-receipt --binary $SourceBinary --target-triple $TargetTriple --receipt $ReceiptPath
if ($LASTEXITCODE -ne 0) {
    throw "sidecar build receipt validation failed"
}
Import-Module $transactionModule -Force

$extension = if ($TargetTriple -match "-windows-") { ".exe" } else { "" }
$binaryDirectory = Join-Path $projectRoot "desktop\src-tauri\binaries"
New-Item -ItemType Directory -Force -Path $binaryDirectory | Out-Null
$targetBinary = Join-Path $binaryDirectory "callerinterview-sidecar-$TargetTriple$extension"
$targetProvenance = "$targetBinary.provenance.json"
$stageToken = "$PID-$([guid]::NewGuid().ToString('N'))"
$stagedBinary = Join-Path $binaryDirectory "callerinterview-sidecar-$TargetTriple.staged-$stageToken$extension"
$stagedProvenance = "$stagedBinary.provenance.json"
$published = $false

try {
    if ($FaultInjection -eq "copy") {
        throw "fault injection: staged copy failed"
    }
    Copy-Item -LiteralPath $SourceBinary -Destination $stagedBinary -Force
    Copy-Item -LiteralPath $ReceiptPath -Destination $stagedProvenance -Force
    & $smokeScript -TargetTriple $TargetTriple -PythonPath $python -BinaryPath $stagedBinary -ProvenancePath $stagedProvenance -StartupBudgetMs 120000
    if ($LASTEXITCODE -ne 0) {
        throw "sidecar staged verification failed"
    }

    $verifyFinal = {
        & $smokeScript -TargetTriple $TargetTriple -PythonPath $python -StartupBudgetMs 120000
        if ($LASTEXITCODE -ne 0) {
            throw "sidecar final verification failed"
        }
    }
    Invoke-SidecarArtifactPairTransaction `
        -CandidateBinary $stagedBinary `
        -CandidateProvenance $stagedProvenance `
        -TargetBinary $targetBinary `
        -TargetProvenance $targetProvenance `
        -VerifyFinal $verifyFinal `
        -FaultInjection $FaultInjection
    $published = $true
}
finally {
    Remove-Item -LiteralPath $stagedBinary -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $stagedProvenance -Force -ErrorAction SilentlyContinue
    if ($published) {
        foreach ($staleFile in @(Get-ChildItem $binaryDirectory -Filter "callerinterview-sidecar-*" -File)) {
            if ($staleFile.FullName -notin @($targetBinary, $targetProvenance)) {
                Remove-Item -LiteralPath $staleFile.FullName -Force
            }
        }
    }
}

Write-Output $targetBinary
