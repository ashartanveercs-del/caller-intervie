[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$modulePath = Join-Path $PSScriptRoot "sidecar-artifact-transaction.psm1"
$fixtureRoot = Join-Path $env:TEMP "callerinterview-sidecar-transaction-$PID"
New-Item -ItemType Directory -Force -Path $fixtureRoot | Out-Null
try {
    Import-Module $modulePath -Force
    $targetBinary = Join-Path $fixtureRoot "target.bin"
    $targetProvenance = Join-Path $fixtureRoot "target.provenance.json"
    $candidateBinary = Join-Path $fixtureRoot "candidate.bin"
    $candidateProvenance = Join-Path $fixtureRoot "candidate.provenance.json"
    [IO.File]::WriteAllText($targetBinary, "old-binary")
    [IO.File]::WriteAllText($targetProvenance, "old-provenance")
    [IO.File]::WriteAllText($candidateBinary, "candidate-binary")
    [IO.File]::WriteAllText($candidateProvenance, "candidate-provenance")

    try {
        Invoke-SidecarArtifactPairTransaction `
            -CandidateBinary $candidateBinary `
            -CandidateProvenance $candidateProvenance `
            -TargetBinary $targetBinary `
            -TargetProvenance $targetProvenance `
            -VerifyFinal { throw "verification should not run after injected failure" } `
            -FaultInjection "after-both-swap"
        throw "transaction accepted post-swap fault injection"
    }
    catch {
        if ($_ -notmatch "fault injection") {
            throw
        }
    }

    if ([IO.File]::ReadAllText($targetBinary) -cne "old-binary") {
        throw "post-swap rollback did not restore the prior binary"
    }
    if ([IO.File]::ReadAllText($targetProvenance) -cne "old-provenance") {
        throw "post-swap rollback did not restore the prior provenance"
    }
}
finally {
    Remove-Item -LiteralPath $fixtureRoot -Recurse -Force -ErrorAction SilentlyContinue
}
