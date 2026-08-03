[CmdletBinding()]
param(
    [ValidateSet("healthcheck", "self-test")]
    [string]$Mode = "healthcheck",
    [string]$TargetTriple = $env:SIDECAR_TARGET_TRIPLE,
    [string]$RustcPath = $env:SIDECAR_RUSTC,
    [string]$PythonPath = $env:SIDECAR_PYTHON,
    [string]$BinaryPath,
    [string]$ProvenancePath,
    [int]$StartupBudgetMs = 30000
)

$ErrorActionPreference = "Stop"

function Resolve-Rustc([string]$configuredPath) {
    if ($configuredPath) {
        if (-not (Test-Path -LiteralPath $configuredPath -PathType Leaf)) {
            throw "RUSTC override does not exist: $configuredPath"
        }
        return (Resolve-Path -LiteralPath $configuredPath).Path
    }
    $command = Get-Command rustc -CommandType Application -ErrorAction SilentlyContinue
    if (-not $command) {
        throw "RUSTC was not found. Set SIDECAR_RUSTC, pass -RustcPath, or pass -TargetTriple."
    }
    return $command.Source
}

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
if (-not $TargetTriple) {
    $rustc = Resolve-Rustc $RustcPath
    $hostLine = & $rustc -vV | Where-Object { $_ -match "^host:\s+(.+)$" } | Select-Object -First 1
    if (-not $hostLine) {
        throw "Unable to determine Rust host target"
    }
    $TargetTriple = [regex]::Match($hostLine, "^host:\s+(.+)$").Groups[1].Value
}

$extension = if ($TargetTriple -match "-windows-") { ".exe" } else { "" }
$binaryDirectory = Join-Path $projectRoot "desktop\src-tauri\binaries"
$expectedName = "callerinterview-sidecar-$TargetTriple$extension"
$expectedBinary = Join-Path $binaryDirectory $expectedName
$binary = if ($BinaryPath) {
    if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
        throw "sidecar staged binary missing: $BinaryPath"
    }
    (Resolve-Path -LiteralPath $BinaryPath).Path
}
else {
    $binaries = if (Test-Path -LiteralPath $binaryDirectory -PathType Container) {
        @(Get-ChildItem $binaryDirectory -Filter "callerinterview-sidecar-*" -File | Where-Object {
            $_.Name -notlike "*.provenance.json" -and
            $_.Name -notmatch "\.(staged|backup|restore-discard)-"
        })
    }
    else {
        @()
    }
    if ($binaries.Count -eq 0) {
        throw "sidecar binary missing: $expectedName"
    }
    if ($binaries.Count -ne 1) {
        throw "sidecar binary ambiguity: expected only $expectedName"
    }
    if ($binaries[0].Name -cne $expectedName -or -not (Test-Path -LiteralPath $expectedBinary -PathType Leaf)) {
        throw "sidecar binary mismatch: expected $expectedName"
    }
    $expectedBinary
}
$provenance = if ($ProvenancePath) { $ProvenancePath } else { "$binary.provenance.json" }
if (-not (Test-Path -LiteralPath $provenance -PathType Leaf)) {
    throw "sidecar provenance missing: $provenance"
}
$python = Resolve-Python $PythonPath $projectRoot
$provenanceHelper = Join-Path $projectRoot "sidecar\package_provenance.py"
& $python $provenanceHelper validate-receipt --binary $binary --target-triple $TargetTriple --receipt $provenance
if ($LASTEXITCODE -ne 0) {
    throw "sidecar provenance validation failed"
}

$arguments = if ($Mode -eq "self-test") { "--self-test" } else { "--healthcheck" }
$stopwatch = [Diagnostics.Stopwatch]::StartNew()
$output = & $binary $arguments
$exitCode = $LASTEXITCODE
$stopwatch.Stop()
if ($exitCode -ne 0) {
    throw "sidecar $Mode failed with exit code $exitCode"
}

$result = $output | ConvertFrom-Json
if ($Mode -eq "healthcheck") {
    if ($result.status -ne "ok" -or $result.protocol_version -ne 1) {
        throw "sidecar healthcheck failed"
    }
}
elseif ($result.status -ne "ok" -or $result.model_revision -ne "1110a243fdf4706b3f48f1d95db1a4f5529b4d41" -or $result.embedding_dimension -ne 384 -or $result.faiss_top_index -ne 0) {
    throw "sidecar frozen self-test failed"
}

$elapsedMs = [math]::Round($stopwatch.Elapsed.TotalMilliseconds, 0)
if ($elapsedMs -gt $StartupBudgetMs) {
    throw "sidecar $Mode exceeded startup budget: ${elapsedMs}ms > ${StartupBudgetMs}ms"
}
Write-Output "sidecar smoke: mode=$Mode target=$TargetTriple startup_ms=$elapsedMs budget_ms=$StartupBudgetMs"
