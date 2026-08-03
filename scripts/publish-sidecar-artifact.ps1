[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SourceBinary,
    [Parameter(Mandatory = $true)]
    [string]$TargetTriple,
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

function Replace-SameVolumeFile([string]$source, [string]$destination, [string]$backup) {
    if (-not (Test-Path -LiteralPath $destination -PathType Leaf)) {
        Move-Item -LiteralPath $source -Destination $destination
        return
    }
    try {
        [IO.File]::Replace($source, $destination, $backup, $true)
    }
    catch [PlatformNotSupportedException] {
        Copy-Item -LiteralPath $destination -Destination $backup -Force
        Move-Item -LiteralPath $source -Destination $destination -Force
    }
}

function Restore-BackupFile([string]$backup, [string]$destination) {
    if (-not (Test-Path -LiteralPath $backup -PathType Leaf)) {
        return
    }
    if (Test-Path -LiteralPath $destination -PathType Leaf) {
        $discard = "$backup.restore-discard-$PID"
        Replace-SameVolumeFile $backup $destination $discard
        Remove-Item -LiteralPath $discard -Force -ErrorAction SilentlyContinue
    }
    else {
        Move-Item -LiteralPath $backup -Destination $destination
    }
}

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$python = Resolve-Python $PythonPath $projectRoot
$smokeScript = Join-Path $PSScriptRoot "test-sidecar-package.ps1"
$provenanceHelper = Join-Path $projectRoot "sidecar\package_provenance.py"
if (-not (Test-Path -LiteralPath $SourceBinary -PathType Leaf)) {
    throw "sidecar source binary missing: $SourceBinary"
}
if ($FaultInjection -and $FaultInjection -notin @("copy", "replace")) {
    throw "unknown sidecar publish fault injection: $FaultInjection"
}

$extension = if ($TargetTriple -match "-windows-") { ".exe" } else { "" }
$binaryDirectory = Join-Path $projectRoot "desktop\src-tauri\binaries"
New-Item -ItemType Directory -Force -Path $binaryDirectory | Out-Null
$targetBinary = Join-Path $binaryDirectory "callerinterview-sidecar-$TargetTriple$extension"
$targetProvenance = "$targetBinary.provenance.json"
$stageToken = "$PID-$([guid]::NewGuid().ToString('N'))"
$stagedBinary = Join-Path $binaryDirectory "callerinterview-sidecar-$TargetTriple.staged-$stageToken$extension"
$stagedProvenance = "$stagedBinary.provenance.json"
$binaryBackup = "$targetBinary.backup-$stageToken"
$provenanceBackup = "$targetProvenance.backup-$stageToken"
$hadBinary = Test-Path -LiteralPath $targetBinary -PathType Leaf
$hadProvenance = Test-Path -LiteralPath $targetProvenance -PathType Leaf
$binaryReplaced = $false
$provenanceReplaced = $false
$published = $false

try {
    if ($FaultInjection -eq "copy") {
        throw "fault injection: staged copy failed"
    }
    Copy-Item -LiteralPath $SourceBinary -Destination $stagedBinary -Force
    & $python $provenanceHelper write --binary $stagedBinary --target-triple $TargetTriple --provenance $stagedProvenance
    if ($LASTEXITCODE -ne 0) {
        throw "sidecar staged provenance generation failed"
    }
    & $smokeScript -TargetTriple $TargetTriple -PythonPath $python -BinaryPath $stagedBinary -ProvenancePath $stagedProvenance -StartupBudgetMs 120000
    if ($LASTEXITCODE -ne 0) {
        throw "sidecar staged verification failed"
    }

    Replace-SameVolumeFile $stagedBinary $targetBinary $binaryBackup
    $binaryReplaced = $true
    if ($FaultInjection -eq "replace") {
        throw "fault injection: replacement failed after binary swap"
    }
    Replace-SameVolumeFile $stagedProvenance $targetProvenance $provenanceBackup
    $provenanceReplaced = $true
    & $smokeScript -TargetTriple $TargetTriple -PythonPath $python -StartupBudgetMs 120000
    if ($LASTEXITCODE -ne 0) {
        throw "sidecar final verification failed"
    }
    $published = $true
}
catch {
    $publishError = $_
    try {
        if ($provenanceReplaced) {
            if ($hadProvenance) {
                Restore-BackupFile $provenanceBackup $targetProvenance
            }
            else {
                Remove-Item -LiteralPath $targetProvenance -Force -ErrorAction SilentlyContinue
            }
        }
        if ($binaryReplaced) {
            if ($hadBinary) {
                Restore-BackupFile $binaryBackup $targetBinary
            }
            else {
                Remove-Item -LiteralPath $targetBinary -Force -ErrorAction SilentlyContinue
            }
        }
    }
    catch {
        throw "sidecar publish failed and rollback failed: $publishError / $_"
    }
    throw $publishError
}
finally {
    Remove-Item -LiteralPath $stagedBinary -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $stagedProvenance -Force -ErrorAction SilentlyContinue
    if ($published) {
        Remove-Item -LiteralPath $binaryBackup -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $provenanceBackup -Force -ErrorAction SilentlyContinue
        foreach ($staleFile in @(Get-ChildItem $binaryDirectory -Filter "callerinterview-sidecar-*" -File)) {
            if ($staleFile.FullName -notin @($targetBinary, $targetProvenance)) {
                Remove-Item -LiteralPath $staleFile.FullName -Force
            }
        }
    }
}

Write-Output $targetBinary
