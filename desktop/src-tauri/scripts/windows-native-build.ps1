[CmdletBinding()]
param(
    [switch] $ProvisionOnly,

    [string[]] $CargoArguments = @()
)

$ErrorActionPreference = 'Stop'
$manifestRoot = Split-Path -Parent $PSScriptRoot
$modulePath = Join-Path $PSScriptRoot 'windows-native-build.psm1'
Import-Module $modulePath -Force

try {
    $exitCode = Invoke-WindowsNativeBuild -ManifestRoot $manifestRoot -ProvisionOnly:$ProvisionOnly -CargoArguments $CargoArguments
    exit $exitCode
}
catch {
    Write-Error $_
    exit 1
}
