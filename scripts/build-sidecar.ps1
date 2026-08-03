[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$python = Join-Path $projectRoot ".venv\Scripts\python.exe"
$rustc = "C:\Users\Admin\.cargo\bin\rustc.exe"
$spec = Join-Path $projectRoot "sidecar\callerinterview-sidecar.spec"
$buildRoot = Join-Path $projectRoot "sidecar\build"
$distRoot = Join-Path $buildRoot "dist"
$binaryDirectory = Join-Path $projectRoot "desktop\src-tauri\binaries"

if (-not (Test-Path -LiteralPath $python -PathType Leaf)) {
    throw "Python environment missing: $python"
}
if (-not (Test-Path -LiteralPath $rustc -PathType Leaf)) {
    throw "rustc missing: $rustc"
}

$hostLine = & $rustc -vV | Where-Object { $_ -match "^host:\s+(.+)$" } | Select-Object -First 1
if (-not $hostLine) {
    throw "Unable to determine Rust host target"
}
$targetTriple = [regex]::Match($hostLine, "^host:\s+(.+)$").Groups[1].Value
$extension = if ($targetTriple -match "-windows-") { ".exe" } else { "" }

& $python -m PyInstaller --noconfirm --clean --workpath (Join-Path $buildRoot "work") --distpath $distRoot $spec
if ($LASTEXITCODE -ne 0) {
    throw "PyInstaller failed with exit code $LASTEXITCODE"
}

$builtBinary = Join-Path $distRoot "callerinterview-sidecar$extension"
if (-not (Test-Path -LiteralPath $builtBinary -PathType Leaf)) {
    throw "PyInstaller output missing: $builtBinary"
}

New-Item -ItemType Directory -Force -Path $binaryDirectory | Out-Null
$targetBinary = Join-Path $binaryDirectory "callerinterview-sidecar-$targetTriple$extension"
Copy-Item -LiteralPath $builtBinary -Destination $targetBinary -Force
Write-Output $targetBinary
