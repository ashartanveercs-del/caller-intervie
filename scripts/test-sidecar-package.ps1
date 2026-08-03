$binary = Get-ChildItem "$PSScriptRoot\..\desktop\src-tauri\binaries\callerinterview-sidecar-*" -File | Select-Object -First 1
if (-not $binary) { throw "sidecar binary missing" }
$result = & $binary.FullName --healthcheck | ConvertFrom-Json
if ($result.status -ne "ok" -or $result.protocol_version -ne 1) { throw "sidecar healthcheck failed" }
