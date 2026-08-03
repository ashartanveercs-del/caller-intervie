[CmdletBinding()]
param(
    [string]$PythonPath = $env:SIDECAR_PYTHON,
    [string]$RustcPath = $env:SIDECAR_RUSTC,
    [string]$TargetTriple = $env:SIDECAR_TARGET_TRIPLE,
    [string]$UpxPath = $env:SIDECAR_UPX,
    [string]$UpxVersion = $env:SIDECAR_UPX_VERSION,
    [switch]$PrepareOnly
)

$ErrorActionPreference = "Stop"

function Resolve-ToolPath(
    [string]$configuredPath,
    [string[]]$venvCandidates,
    [string[]]$commandNames,
    [string]$label
) {
    if ($configuredPath) {
        if (-not (Test-Path -LiteralPath $configuredPath -PathType Leaf)) {
            throw "$label override does not exist: $configuredPath"
        }
        return (Resolve-Path -LiteralPath $configuredPath).Path
    }

    foreach ($candidate in $venvCandidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }

    foreach ($commandName in $commandNames) {
        $command = Get-Command $commandName -CommandType Application -ErrorAction SilentlyContinue
        if ($command) {
            return $command.Source
        }
    }

    throw "$label was not found. Set SIDECAR_$label or pass the corresponding parameter."
}

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$spec = Join-Path $projectRoot "sidecar\callerinterview-sidecar.spec"
$buildRoot = Join-Path $projectRoot "sidecar\build"
$distRoot = Join-Path $buildRoot "dist"
$modelAssets = Join-Path $buildRoot "rag-model"
$modelFetcher = Join-Path $projectRoot "sidecar\fetch_model_assets.py"
$publisher = Join-Path $projectRoot "scripts\publish-sidecar-artifact.ps1"

$python = Resolve-ToolPath $PythonPath @(
    (Join-Path $projectRoot ".venv\Scripts\python.exe"),
    (Join-Path $projectRoot ".venv/bin/python")
) @("python3", "python") "PYTHON"

if (-not $TargetTriple) {
    $rustc = Resolve-ToolPath $RustcPath @() @("rustc") "RUSTC"
    $hostLine = & $rustc -vV | Where-Object { $_ -match "^host:\s+(.+)$" } | Select-Object -First 1
    if (-not $hostLine) {
        throw "Unable to determine Rust host target"
    }
    $TargetTriple = [regex]::Match($hostLine, "^host:\s+(.+)$").Groups[1].Value
}

if ($UpxPath) {
    if (-not $UpxVersion) {
        throw "UPX requires an explicit SIDECAR_UPX_VERSION or -UpxVersion pin"
    }
    if (-not (Test-Path -LiteralPath $UpxPath -PathType Leaf)) {
        throw "UPX override does not exist: $UpxPath"
    }
    $upxOutput = (& $UpxPath --version | Out-String)
    if ($LASTEXITCODE -ne 0 -or $upxOutput -notmatch [regex]::Escape($UpxVersion)) {
        throw "UPX version does not match the supplied pin: $UpxVersion"
    }
    $env:Path = "$(Split-Path -Parent $UpxPath)$([IO.Path]::PathSeparator)$env:Path"
    $env:SIDECAR_USE_UPX = "1"
    Write-Output "sidecar build: upx=$UpxPath version=$UpxVersion"
}
else {
    $env:SIDECAR_USE_UPX = "0"
    Write-Output "sidecar build: upx=disabled"
}

$extension = if ($TargetTriple -match "-windows-") { ".exe" } else { "" }
Write-Output "sidecar build: python=$python target=$TargetTriple"

foreach ($outputDirectory in @(
    (Join-Path $buildRoot "dist"),
    (Join-Path $buildRoot "work")
)) {
    if (Test-Path -LiteralPath $outputDirectory -PathType Container) {
        Remove-Item -LiteralPath $outputDirectory -Recurse -Force
    }
}
& $python $modelFetcher --output $modelAssets
if ($LASTEXITCODE -ne 0) {
    throw "Pinned RAG model fetch failed with exit code $LASTEXITCODE"
}
if ($PrepareOnly) {
    Write-Output "sidecar build: prepared pinned model assets at $modelAssets"
    return
}

& $python -m PyInstaller --noconfirm --clean --workpath (Join-Path $buildRoot "work") --distpath $distRoot $spec
if ($LASTEXITCODE -ne 0) {
    throw "PyInstaller failed with exit code $LASTEXITCODE"
}

$builtBinary = Join-Path $distRoot "callerinterview-sidecar$extension"
if (-not (Test-Path -LiteralPath $builtBinary -PathType Leaf)) {
    throw "PyInstaller output missing: $builtBinary"
}

& $publisher -SourceBinary $builtBinary -TargetTriple $TargetTriple -PythonPath $python
if ($LASTEXITCODE -ne 0) {
    throw "sidecar artifact publish failed with exit code $LASTEXITCODE"
}
