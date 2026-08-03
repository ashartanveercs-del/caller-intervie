Set-StrictMode -Version Latest

function Replace-SameVolumeFile {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][string]$Backup
    )

    if (-not (Test-Path -LiteralPath $Destination -PathType Leaf)) {
        Move-Item -LiteralPath $Source -Destination $Destination
        return
    }
    try {
        [IO.File]::Replace($Source, $Destination, $Backup, $true)
    }
    catch [PlatformNotSupportedException] {
        Copy-Item -LiteralPath $Destination -Destination $Backup -Force
        Move-Item -LiteralPath $Source -Destination $Destination -Force
    }
}

function Restore-BackupFile {
    param(
        [Parameter(Mandatory = $true)][string]$Backup,
        [Parameter(Mandatory = $true)][string]$Destination
    )

    if (-not (Test-Path -LiteralPath $Backup -PathType Leaf)) {
        return
    }
    if (Test-Path -LiteralPath $Destination -PathType Leaf) {
        $discard = "$Backup.restore-discard-$PID"
        Replace-SameVolumeFile -Source $Backup -Destination $Destination -Backup $discard
        Remove-Item -LiteralPath $discard -Force -ErrorAction SilentlyContinue
    }
    else {
        Move-Item -LiteralPath $Backup -Destination $Destination
    }
}

function Invoke-SidecarArtifactPairTransaction {
    param(
        [Parameter(Mandatory = $true)][string]$CandidateBinary,
        [Parameter(Mandatory = $true)][string]$CandidateProvenance,
        [Parameter(Mandatory = $true)][string]$TargetBinary,
        [Parameter(Mandatory = $true)][string]$TargetProvenance,
        [Parameter(Mandatory = $true)][scriptblock]$VerifyFinal,
        [ValidateSet("", "before-first-swap", "after-both-swap")][string]$FaultInjection = ""
    )

    $hadBinary = Test-Path -LiteralPath $TargetBinary -PathType Leaf
    $hadProvenance = Test-Path -LiteralPath $TargetProvenance -PathType Leaf
    if ($hadBinary -ne $hadProvenance) {
        throw "sidecar target binary and provenance must be replaced as a pair"
    }

    $token = "$PID-$([guid]::NewGuid().ToString('N'))"
    $binaryBackup = "$TargetBinary.backup-$token"
    $provenanceBackup = "$TargetProvenance.backup-$token"
    $binarySwapped = $false
    $provenanceSwapped = $false
    $published = $false

    try {
        if ($FaultInjection -eq "before-first-swap") {
            throw "fault injection: pair replacement failed before first swap"
        }
        Replace-SameVolumeFile -Source $CandidateBinary -Destination $TargetBinary -Backup $binaryBackup
        $binarySwapped = $true
        Replace-SameVolumeFile -Source $CandidateProvenance -Destination $TargetProvenance -Backup $provenanceBackup
        $provenanceSwapped = $true
        if ($FaultInjection -eq "after-both-swap") {
            throw "fault injection: pair replacement failed after both swaps"
        }
        & $VerifyFinal
        $published = $true
    }
    catch {
        $transactionError = $_
        try {
            if ($provenanceSwapped) {
                if ($hadProvenance) {
                    Restore-BackupFile -Backup $provenanceBackup -Destination $TargetProvenance
                }
                else {
                    Remove-Item -LiteralPath $TargetProvenance -Force -ErrorAction SilentlyContinue
                }
            }
            if ($binarySwapped) {
                if ($hadBinary) {
                    Restore-BackupFile -Backup $binaryBackup -Destination $TargetBinary
                }
                else {
                    Remove-Item -LiteralPath $TargetBinary -Force -ErrorAction SilentlyContinue
                }
            }
        }
        catch {
            throw "sidecar pair replacement failed and rollback failed: $transactionError / $_"
        }
        throw $transactionError
    }
    finally {
        if ($published) {
            Remove-Item -LiteralPath $binaryBackup -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath $provenanceBackup -Force -ErrorAction SilentlyContinue
        }
    }
}

Export-ModuleMember -Function Invoke-SidecarArtifactPairTransaction
