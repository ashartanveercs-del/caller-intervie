$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$script:BuildContract = [pscustomobject]@{
    SchemaVersion = 1
    RecipeRevision = 3
    LibsodiumVersion = '1.0.20'
    SourceAsset = 'libsodium-1.0.20.tar.gz'
    SourceUrl = 'https://github.com/jedisct1/libsodium/releases/download/1.0.20-RELEASE/libsodium-1.0.20.tar.gz'
    SourceSha256 = 'ebb65ef6ca439333c2bb41a0c1990587288da07f6c7fd07cb3a18cc18d30ce19'
    SourceDirectory = 'libsodium-1.0.20'
    Solution = 'builds/msvc/vs2022/libsodium.sln'
    Configuration = 'StaticRelease'
    Platform = 'x64'
    PlatformToolset = 'v143'
    RuntimeLibrary = 'MultiThreadedDLL'
    PropertyFile = 'builds/msvc/properties/ReleaseLIB.props'
    DebugPropertyFile = 'builds/msvc/properties/Release.props'
    DebugInformationFormat = 'OldStyle'
    Library = 'lib/libsodium.lib'
    CacheDirectory = 'libsodium-1.0.20-msvc-static-md-x64'
    CargoTarget = 'x86_64-pc-windows-msvc'
    VisualStudioInstallationVersion = '17.14.37516.0'
    MsBuildVersion = '17.14.51.32402'
    VcToolsVersion = '14.44.35207'
    LinkVersion = '14.44.35228.0'
    OpenSslVersion = '3.6.3'
    OpenSslSourcePackageVersion = '300.6.1+3.6.3'
    OpenSslSourcePackageChecksum = '46eb8fb9fb3b61ce1c0f8a026c4c1a0714d3a9e138e7fbde78753ce2babc3846'
    OpenSslWindowsMakefileSha256 = 'e1a8d9a44afb425c84a02e98d68527bcbdf098efbcd614445b596b210ba829d2'
    OpenSslPatchedWindowsMakefileSha256 = 'b6d42fcf197b702217439a810a8a12a3173297b5246c8613146131dc2d875c62'
}

function Get-WindowsNativeBuildContract {
    return $script:BuildContract
}

function Convert-LibsodiumReleasePropertyToDynamicCrt {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [string] $Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "The libsodium ReleaseLIB property file is missing: $Path"
    }

    $staticNode = '<RuntimeLibrary>MultiThreaded</RuntimeLibrary>'
    $dynamicNode = '<RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary>'
    $content = [System.IO.File]::ReadAllText($Path)
    $staticCount = [regex]::Matches($content, [regex]::Escape($staticNode)).Count
    $dynamicCount = [regex]::Matches($content, [regex]::Escape($dynamicNode)).Count

    if ($staticCount -ne 1 -or $dynamicCount -ne 0) {
        throw "Expected exactly one MultiThreaded RuntimeLibrary node and no MultiThreadedDLL node; found $staticCount and $dynamicCount."
    }

    $content = $content.Replace($staticNode, $dynamicNode)
    $remainingStaticCount = [regex]::Matches($content, [regex]::Escape($staticNode)).Count
    $resultingDynamicCount = [regex]::Matches($content, [regex]::Escape($dynamicNode)).Count
    if ($remainingStaticCount -ne 0 -or $resultingDynamicCount -ne 1) {
        throw "The verified RuntimeLibrary transform did not produce exactly one MultiThreadedDLL node."
    }

    [System.IO.File]::WriteAllText($Path, $content, [System.Text.UTF8Encoding]::new($true))
    return 1
}

function Convert-LibsodiumReleaseDebugInformationToEmbedded {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [string] $Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "The libsodium Release property file is missing: $Path"
    }

    $externalNode = '<DebugInformationFormat>ProgramDatabase</DebugInformationFormat>'
    $embeddedNode = '<DebugInformationFormat>OldStyle</DebugInformationFormat>'
    $content = [System.IO.File]::ReadAllText($Path)
    $externalCount = [regex]::Matches($content, [regex]::Escape($externalNode)).Count
    $embeddedCount = [regex]::Matches($content, [regex]::Escape($embeddedNode)).Count
    if ($externalCount -ne 1 -or $embeddedCount -ne 0) {
        throw "Expected exactly one ProgramDatabase DebugInformationFormat node and no OldStyle node; found $externalCount and $embeddedCount."
    }

    $content = $content.Replace($externalNode, $embeddedNode)
    [System.IO.File]::WriteAllText($Path, $content, [System.Text.UTF8Encoding]::new($true))
    return 1
}

function Convert-OpenSslWindowsMakefileToEmbeddedDebug {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [string] $SourceTemplatePath,

        [Parameter(Mandatory = $true)]
        [string] $DestinationTemplatePath
    )

    if (-not (Test-Path -LiteralPath $SourceTemplatePath -PathType Leaf)) {
        throw "The pinned OpenSSL Windows makefile template is missing: $SourceTemplatePath"
    }

    $content = [System.IO.File]::ReadAllText($SourceTemplatePath)
    $externalDebugFlags = 'LIB_CFLAGS={- join('' '', $target{lib_cflags} || (),'
    $embeddedDebugFlags = @'
LIB_CFLAGS={- die "Unexpected OpenSSL static-library flags for VC-WIN64A"
                    unless $config{target} eq "VC-WIN64A"
                        && ($target{lib_cflags} // "") eq "/Zi /Fdossl_static.pdb /MT /Zl";
                join(' ', '/Z7 /MT /Zl',
'@
    $externalDebugCount = [regex]::Matches(
        $content,
        [regex]::Escape($externalDebugFlags)
    ).Count
    $pdbInstallPattern = '(?m)^\t@if "\$\(SHLIBS\)"=="" \\\r?\n\t "\$\(PERL\)" "\$\(SRCDIR\)\\util\\copy\.pl" ossl_static\.pdb "\$\(libdir\)"\r?\n?'
    $pdbInstallCount = [regex]::Matches($content, $pdbInstallPattern).Count

    if ($externalDebugCount -ne 1 -or $pdbInstallCount -ne 1) {
        throw "OpenSSL Windows makefile template drift detected; expected one static debug-flag block and one static PDB install command, found $externalDebugCount and $pdbInstallCount."
    }

    $content = $content.Replace($externalDebugFlags, $embeddedDebugFlags)
    $content = [regex]::Replace(
        $content,
        $pdbInstallPattern,
        "# Static-library debug data is embedded by /Z7; no compiler PDB is installed.$([Environment]::NewLine)"
    )

    $destinationDirectory = Split-Path -Parent $DestinationTemplatePath
    New-Item -ItemType Directory -Path $destinationDirectory -Force | Out-Null
    $temporaryPath = "$DestinationTemplatePath.tmp.$([guid]::NewGuid().ToString('N'))"
    try {
        [System.IO.File]::WriteAllText(
            $temporaryPath,
            $content,
            [System.Text.UTF8Encoding]::new($false)
        )
        Move-Item -LiteralPath $temporaryPath -Destination $DestinationTemplatePath -Force
    }
    finally {
        Remove-Item -LiteralPath $temporaryPath -Force -ErrorAction SilentlyContinue
    }

    return 2
}

function Assert-PinnedOpenSslCargoPackage {
    param(
        [Parameter(Mandatory = $true)] [string] $CargoLockPath,
        [Parameter(Mandatory = $true)] [string] $PackageVersion,
        [Parameter(Mandatory = $true)] [string] $PackageChecksum
    )

    if (-not (Test-Path -LiteralPath $CargoLockPath -PathType Leaf)) {
        throw "Cargo.lock is missing: $CargoLockPath"
    }

    $content = [System.IO.File]::ReadAllText($CargoLockPath)
    $packageBlocks = @([regex]::Matches(
            $content,
            '(?ms)^\[\[package\]\]\r?\n.*?(?=^\[\[package\]\]|\z)'
        ) | Where-Object { $_.Value -match '(?m)^name = "openssl-src"\r?$' })
    if ($packageBlocks.Count -ne 1) {
        throw "Cargo.lock must contain exactly one openssl-src package; found $($packageBlocks.Count)."
    }

    $block = $packageBlocks[0].Value
    $expectedVersionLine = "version = `"$PackageVersion`""
    $expectedChecksumLine = "checksum = `"$PackageChecksum`""
    if ($block -notmatch "(?m)^$([regex]::Escape($expectedVersionLine))\r?$" -or
        $block -notmatch "(?m)^$([regex]::Escape($expectedChecksumLine))\r?$") {
        throw "Cargo.lock does not pin the approved openssl-src $PackageVersion package and checksum."
    }
}

function Get-DefaultCargoHomePath {
    if ($env:CARGO_HOME) {
        return [System.IO.Path]::GetFullPath($env:CARGO_HOME)
    }
    return Join-Path ([Environment]::GetFolderPath('UserProfile')) '.cargo'
}

function Get-PinnedOpenSslWindowsMakefileTemplate {
    param(
        [Parameter(Mandatory = $true)] [string] $CargoPath,
        [Parameter(Mandatory = $true)] [string] $ManifestRoot,
        [Parameter(Mandatory = $true)] [string] $CargoHomePath,
        [Parameter(Mandatory = $true)] [string] $PackageVersion,
        [Parameter(Mandatory = $true)] [string] $ExpectedTemplateSha256
    )

    if ($ExpectedTemplateSha256 -notmatch '^[0-9a-fA-F]{64}$') {
        throw 'The approved OpenSSL Windows template hash is invalid.'
    }

    $findCandidates = {
        $registrySourceRoot = Join-Path $CargoHomePath 'registry\src'
        if (-not (Test-Path -LiteralPath $registrySourceRoot -PathType Container)) {
            return
        }
        foreach ($registryDirectory in @(Get-ChildItem -LiteralPath $registrySourceRoot -Directory -ErrorAction Stop)) {
            $candidate = Join-Path $registryDirectory.FullName "openssl-src-$PackageVersion\openssl\Configurations\windows-makefile.tmpl"
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                $candidate
            }
        }
    }

    $candidates = @(& $findCandidates)
    if ($candidates.Count -eq 0) {
        $fetchExitCode = Invoke-NativeCommandWithOutput `
            -ExecutablePath $CargoPath `
            -Arguments @('fetch', '--locked', '--manifest-path', (Join-Path $ManifestRoot 'Cargo.toml'))
        if ($fetchExitCode -ne 0) {
            throw "Unable to fetch the pinned OpenSSL source package (cargo fetch exit $fetchExitCode)."
        }
        $candidates = @(& $findCandidates)
    }

    if ($candidates.Count -eq 0) {
        throw "The pinned openssl-src $PackageVersion Windows template was not found under $CargoHomePath."
    }

    $matchingCandidates = @($candidates | Where-Object {
            (Get-FileHash -Algorithm SHA256 -LiteralPath $_).Hash.ToLowerInvariant() -ceq $ExpectedTemplateSha256.ToLowerInvariant()
        } | Sort-Object)
    if ($matchingCandidates.Count -eq 0) {
        throw "OpenSSL Windows makefile template hash mismatch for cached openssl-src $PackageVersion; refusing source drift."
    }
    return [string] $matchingCandidates[0]
}

function Initialize-OpenSslLocalConfiguration {
    param(
        [Parameter(Mandatory = $true)] [string] $CargoPath,
        [Parameter(Mandatory = $true)] [string] $ManifestRoot
    )

    $contract = Get-WindowsNativeBuildContract
    Assert-PinnedOpenSslCargoPackage `
        -CargoLockPath (Join-Path $ManifestRoot 'Cargo.lock') `
        -PackageVersion $contract.OpenSslSourcePackageVersion `
        -PackageChecksum $contract.OpenSslSourcePackageChecksum
    $sourceTemplate = Get-PinnedOpenSslWindowsMakefileTemplate `
        -CargoPath $CargoPath `
        -ManifestRoot $ManifestRoot `
        -CargoHomePath (Get-DefaultCargoHomePath) `
        -PackageVersion $contract.OpenSslSourcePackageVersion `
        -ExpectedTemplateSha256 $contract.OpenSslWindowsMakefileSha256
    $templateRevision = $contract.OpenSslPatchedWindowsMakefileSha256.Substring(0, 12)
    $configurationDirectory = Join-Path $ManifestRoot ".native\openssl-config-$($contract.OpenSslVersion)-$templateRevision"
    $destinationTemplate = Join-Path $configurationDirectory 'windows-makefile.tmpl'
    Convert-OpenSslWindowsMakefileToEmbeddedDebug `
        -SourceTemplatePath $sourceTemplate `
        -DestinationTemplatePath $destinationTemplate | Out-Null
    $patchedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $destinationTemplate).Hash.ToLowerInvariant()
    if ($patchedHash -cne $contract.OpenSslPatchedWindowsMakefileSha256) {
        throw "The generated OpenSSL Windows makefile template hash is $patchedHash; expected $($contract.OpenSslPatchedWindowsMakefileSha256)."
    }
    return $configurationDirectory
}

function Assert-LibsodiumArchiveInspection {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [string] $DirectiveOutput,

        [Parameter(Mandatory = $true)]
        [string] $HeaderOutput
    )

    $staticCrtPattern = '(?i)/DEFAULTLIB:"?LIBCMTD?"?(?=\s|$)'
    if ([regex]::IsMatch($DirectiveOutput, $staticCrtPattern)) {
        throw 'libsodium.lib requests LIBCMT or LIBCMTD; refusing the static-CRT archive.'
    }

    $debugDynamicCrtPattern = '(?i)/DEFAULTLIB:"?MSVCRTD"?(?=\s|$)'
    if ([regex]::IsMatch($DirectiveOutput, $debugDynamicCrtPattern)) {
        throw 'libsodium.lib requests MSVCRTD; refusing the debug dynamic-CRT archive.'
    }

    $runtimeMismatches = [regex]::Matches(
        $DirectiveOutput,
        '(?i)/FAILIFMISMATCH:"?RuntimeLibrary=([^"\s]+)"?'
    )
    foreach ($runtimeMismatch in $runtimeMismatches) {
        if ($runtimeMismatch.Groups[1].Value -cne 'MD_DynamicRelease') {
            throw "libsodium.lib contains an incompatible RuntimeLibrary directive: $($runtimeMismatch.Groups[1].Value)."
        }
    }

    $dynamicCrtPattern = '(?i)/DEFAULTLIB:"?MSVCRT"?(?=\s|$)'
    if (-not [regex]::IsMatch($DirectiveOutput, $dynamicCrtPattern)) {
        throw 'libsodium.lib does not request the release dynamic CRT default library MSVCRT.'
    }

    $machineMatches = [regex]::Matches(
        $HeaderOutput,
        '(?im)^\s*[0-9a-f]+\s+machine\s+\(([^)]+)\)\s*$'
    )
    if ($machineMatches.Count -eq 0) {
        throw 'libsodium.lib has no inspectable COFF machine headers.'
    }

    foreach ($machineMatch in $machineMatches) {
        if ($machineMatch.Groups[1].Value -ne 'x64') {
            throw "libsodium.lib contains a non-x64 object: $($machineMatch.Groups[1].Value)."
        }
    }
}

function Get-ReceiptValue {
    param(
        [Parameter(Mandatory = $true)] $Receipt,
        [Parameter(Mandatory = $true)] [string] $Name
    )

    if ($Receipt -is [System.Collections.IDictionary]) {
        if (-not $Receipt.ContainsKey($Name)) {
            throw "The libsodium build receipt is missing '$Name'."
        }
        return $Receipt[$Name]
    }

    $property = $Receipt.PSObject.Properties[$Name]
    if ($null -eq $property) {
        throw "The libsodium build receipt is missing '$Name'."
    }
    return $property.Value
}

function Assert-LibsodiumBuildReceipt {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [string] $ReceiptPath,

        [Parameter(Mandatory = $true)]
        [string] $LibraryPath
    )

    if (-not (Test-Path -LiteralPath $ReceiptPath -PathType Leaf)) {
        throw "The libsodium build receipt is missing: $ReceiptPath"
    }
    if (-not (Test-Path -LiteralPath $LibraryPath -PathType Leaf)) {
        throw "The provisioned libsodium library is missing: $LibraryPath"
    }
    if ((Get-Item -LiteralPath $LibraryPath).Length -eq 0) {
        throw 'The provisioned libsodium library is empty.'
    }
    if ((Get-Item -LiteralPath $ReceiptPath).Length -gt 65536) {
        throw 'The libsodium build receipt is unexpectedly large.'
    }

    try {
        Add-Type -AssemblyName System.Web.Extensions
        $serializer = New-Object System.Web.Script.Serialization.JavaScriptSerializer
        $serializer.MaxJsonLength = 65536
        $receipt = $serializer.DeserializeObject((Get-Content -LiteralPath $ReceiptPath -Raw))
    }
    catch {
        throw "The libsodium build receipt is not valid JSON: $($_.Exception.Message)"
    }

    if ($null -eq $receipt -or $receipt -isnot [System.Collections.IDictionary]) {
        throw 'The libsodium build receipt must be a JSON object.'
    }

    $contract = Get-WindowsNativeBuildContract
    $expectedIntegers = [ordered]@{
        schema_version = $contract.SchemaVersion
        recipe_revision = $contract.RecipeRevision
        property_transform_count = 1
        debug_property_transform_count = 1
    }
    $expectedStrings = [ordered]@{
        libsodium_version = $contract.LibsodiumVersion
        source_asset = $contract.SourceAsset
        source_url = $contract.SourceUrl
        source_sha256 = $contract.SourceSha256
        solution = $contract.Solution
        configuration = $contract.Configuration
        platform = $contract.Platform
        platform_toolset = $contract.PlatformToolset
        runtime_library = $contract.RuntimeLibrary
        property_file = $contract.PropertyFile
        debug_property_file = $contract.DebugPropertyFile
        debug_information_format = $contract.DebugInformationFormat
        library = $contract.Library
        machine = 'x64'
        crt_default_library = 'MSVCRT'
        visual_studio_installation_version = $contract.VisualStudioInstallationVersion
        msbuild_version = $contract.MsBuildVersion
        vc_tools_version = $contract.VcToolsVersion
        link_version = $contract.LinkVersion
    }

    $expectedFieldNames = @($expectedIntegers.Keys) + @($expectedStrings.Keys) + @('library_sha256')
    foreach ($propertyName in $receipt.Keys) {
        if ($expectedFieldNames -cnotcontains $propertyName) {
            throw "The libsodium build receipt contains unknown JSON field '$propertyName'."
        }
    }

    $integerTypeCodes = @(
        [System.TypeCode]::SByte,
        [System.TypeCode]::Byte,
        [System.TypeCode]::Int16,
        [System.TypeCode]::UInt16,
        [System.TypeCode]::Int32,
        [System.TypeCode]::UInt32,
        [System.TypeCode]::Int64,
        [System.TypeCode]::UInt64
    )
    foreach ($entry in $expectedIntegers.GetEnumerator()) {
        $actual = Get-ReceiptValue -Receipt $receipt -Name $entry.Key
        if ($null -eq $actual -or $integerTypeCodes -cnotcontains [System.Type]::GetTypeCode($actual.GetType())) {
            throw "The libsodium build receipt field '$($entry.Key)' must be a JSON unsigned integer."
        }
        $numeric = [decimal] $actual
        if ($numeric -lt 0 -or $numeric -gt [uint32]::MaxValue) {
            throw "The libsodium build receipt field '$($entry.Key)' must be a JSON unsigned integer."
        }
        if ($numeric -ne $entry.Value) {
            throw "The libsodium build receipt field '$($entry.Key)' is stale or incompatible."
        }
    }

    foreach ($entry in $expectedStrings.GetEnumerator()) {
        $actual = Get-ReceiptValue -Receipt $receipt -Name $entry.Key
        if ($actual -isnot [string]) {
            throw "The libsodium build receipt field '$($entry.Key)' must be a JSON string."
        }
        if ($actual -cne $entry.Value) {
            throw "The libsodium build receipt field '$($entry.Key)' is stale or incompatible."
        }
    }

    $expectedLibraryHash = Get-ReceiptValue -Receipt $receipt -Name 'library_sha256'
    if ($expectedLibraryHash -isnot [string]) {
        throw "The libsodium build receipt field 'library_sha256' must be a JSON string."
    }
    if ($expectedLibraryHash -notmatch '^[0-9a-f]{64}$') {
        throw 'The libsodium build receipt library SHA-256 is invalid.'
    }
    $actualLibraryHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $LibraryPath).Hash.ToLowerInvariant()
    if ($actualLibraryHash -cne $expectedLibraryHash) {
        throw "The provisioned libsodium library SHA-256 does not match its receipt."
    }

    return $receipt
}

function Get-VsWherePath {
    $command = Get-Command vswhere.exe -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        return $command.Source
    }

    $programRoots = @(${env:ProgramFiles(x86)}, $env:ProgramFiles) | Where-Object { $_ }
    foreach ($programRoot in $programRoots) {
        $candidate = Join-Path $programRoot 'Microsoft Visual Studio\Installer\vswhere.exe'
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }

    throw 'vswhere.exe was not found. Install Visual Studio 2022 Build Tools with MSVC and MSBuild.'
}

function Get-VisualStudioInstallationPath {
    param(
        [Parameter(Mandatory = $true)] [string] $VsWherePath
    )

    $contract = Get-WindowsNativeBuildContract
    $output = @(& $VsWherePath -latest -version '[17.14,17.15)' -products '*' -requires Microsoft.Component.MSBuild Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -format json -utf8)
    if ($LASTEXITCODE -ne 0) {
        throw "vswhere.exe failed while locating Visual Studio 2022 Build Tools (exit $LASTEXITCODE)."
    }
    try {
        $installations = @((($output -join [Environment]::NewLine) | ConvertFrom-Json))
    }
    catch {
        throw "vswhere.exe returned invalid JSON: $($_.Exception.Message)"
    }
    if ($installations.Count -ne 1) {
        throw "vswhere.exe did not find exactly one approved Visual Studio $($contract.VisualStudioInstallationVersion) installation."
    }

    $installation = $installations[0]
    if ($installation.installationVersion -cne $contract.VisualStudioInstallationVersion) {
        throw "Visual Studio installation version must be exactly $($contract.VisualStudioInstallationVersion); found '$($installation.installationVersion)'."
    }
    if (-not $installation.installationPath -or -not (Test-Path -LiteralPath $installation.installationPath -PathType Container)) {
        throw 'The approved Visual Studio installation path is unavailable.'
    }
    return [System.IO.Path]::GetFullPath($installation.installationPath)
}

function Assert-ApprovedToolchainVersions {
    param(
        [Parameter(Mandatory = $true)] [string] $VisualStudioInstallationVersion,
        [Parameter(Mandatory = $true)] [string] $MsBuildVersion,
        [Parameter(Mandatory = $true)] [string] $VcToolsVersion,
        [Parameter(Mandatory = $true)] [string] $LinkVersion
    )

    $contract = Get-WindowsNativeBuildContract
    foreach ($field in @(
        'VisualStudioInstallationVersion',
        'MsBuildVersion',
        'VcToolsVersion',
        'LinkVersion'
    )) {
        $actual = Get-Variable -Name $field -ValueOnly
        if ($actual -cne $contract.$field) {
            throw "$field must be exactly $($contract.$field); found '$actual'."
        }
    }
}

function Get-VisualStudio2022Toolchain {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)] [string] $InstallationPath
    )

    $installation = [System.IO.Path]::GetFullPath($InstallationPath)
    if (-not (Test-Path -LiteralPath $installation -PathType Container)) {
        throw "The selected Visual Studio 2022 installation is missing: $installation"
    }

    $developerCommand = Join-Path $installation 'Common7\Tools\VsDevCmd.bat'
    $msbuild = Join-Path $installation 'MSBuild\Current\Bin\MSBuild.exe'
    $v143Toolset = Join-Path $installation 'MSBuild\Microsoft\VC\v170\Platforms\x64\PlatformToolsets\v143\Toolset.props'
    foreach ($requiredFile in @(
        @{ Path = $developerCommand; Name = 'VsDevCmd.bat' },
        @{ Path = $msbuild; Name = 'MSBuild.exe' },
        @{ Path = $v143Toolset; Name = 'the x64 v143 platform toolset' }
    )) {
        if (-not (Test-Path -LiteralPath $requiredFile.Path -PathType Leaf)) {
            throw "$($requiredFile.Name) is missing from the selected Visual Studio 2022 installation."
        }
    }

    $contract = Get-WindowsNativeBuildContract
    $vcToolsInstallDirectory = Join-Path $installation "VC\Tools\MSVC\$($contract.VcToolsVersion)"
    $link = Join-Path $vcToolsInstallDirectory 'bin\Hostx64\x64\link.exe'
    if (-not (Test-Path -LiteralPath $link -PathType Leaf)) {
        throw "The selected Visual Studio installation does not contain the approved VC tools directory $($contract.VcToolsVersion)."
    }

    return [pscustomobject]@{
        InstallationPath = $installation
        DeveloperCommand = $developerCommand
        MsBuildPath = $msbuild
        LinkPath = $link
        V143ToolsetPath = $v143Toolset
        VcToolsInstallDirectory = $vcToolsInstallDirectory
    }
}

function Initialize-VisualStudioBuildEnvironment {
    param(
        [Parameter(Mandatory = $true)] [string] $DeveloperCommand
    )

    if (-not (Test-Path -LiteralPath $DeveloperCommand -PathType Leaf)) {
        throw 'The Visual Studio developer environment command was not found.'
    }

    $commandLine = "`"$DeveloperCommand`" -no_logo -arch=x64 -host_arch=x64 >nul && set"
    $environmentLines = @(& $env:ComSpec /d /s /c $commandLine)
    if ($LASTEXITCODE -ne 0) {
        throw "VsDevCmd.bat failed to initialize the x64 build environment (exit $LASTEXITCODE)."
    }

    foreach ($line in $environmentLines) {
        $separator = $line.IndexOf('=')
        if ($separator -le 0) {
            continue
        }
        $name = $line.Substring(0, $separator)
        $value = $line.Substring($separator + 1)
        [System.Environment]::SetEnvironmentVariable($name, $value, 'Process')
    }
}

function Assert-SelectedVisualStudioEnvironment {
    param(
        [Parameter(Mandatory = $true)] $Toolchain
    )

    if ($env:VisualStudioVersion -notmatch '^17(?:\.|$)') {
        throw 'VsDevCmd did not initialize a Visual Studio 2022 version 17.x environment.'
    }
    foreach ($variable in @('VSINSTALLDIR', 'VCToolsInstallDir')) {
        if (-not (Get-Item "Env:$variable" -ErrorAction SilentlyContinue)) {
            throw "VsDevCmd did not set $variable."
        }
    }

    $selectedInstallation = [System.IO.Path]::GetFullPath($Toolchain.InstallationPath).TrimEnd('\')
    $environmentInstallation = [System.IO.Path]::GetFullPath($env:VSINSTALLDIR).TrimEnd('\')
    if ($selectedInstallation -cne $environmentInstallation) {
        throw 'VsDevCmd initialized a different Visual Studio installation than the selected toolchain.'
    }

    $environmentLink = Join-Path $env:VCToolsInstallDir 'bin\Hostx64\x64\link.exe'
    if (-not (Test-Path -LiteralPath $environmentLink -PathType Leaf)) {
        throw 'VsDevCmd selected an MSVC tool directory without the x64 linker.'
    }
    $expectedLink = (Get-Item -LiteralPath $Toolchain.LinkPath).FullName
    $actualLink = (Get-Item -LiteralPath $environmentLink).FullName
    if ($expectedLink -cne $actualLink) {
        throw 'VsDevCmd selected a different MSVC linker than the pinned Visual Studio 2022 toolchain.'
    }
}

function Get-ExecutableVersion {
    param(
        [Parameter(Mandatory = $true)] [string] $ExecutablePath,
        [Parameter(Mandatory = $true)] [string[]] $Arguments,
        [Parameter(Mandatory = $true)] [string] $DisplayName,
        [int[]] $AcceptedExitCodes = @(0)
    )

    $output = @(& $ExecutablePath @Arguments 2>&1)
    $exitCode = $LASTEXITCODE
    if ($AcceptedExitCodes -notcontains $exitCode) {
        throw "$DisplayName version inspection failed (exit $exitCode)."
    }
    foreach ($line in $output) {
        $match = [regex]::Match($line.ToString(), '(?<!\d)(\d+(?:\.\d+)+)(?!\d)')
        if ($match.Success) {
            return $match.Groups[1].Value
        }
    }
    throw "$DisplayName did not report a parseable version."
}

function Get-ApprovedMsvcLinkerVersion {
    param(
        [Parameter(Mandatory = $true)] [string] $LinkPath
    )

    $version = Get-ExecutableVersion `
        -ExecutablePath $LinkPath `
        -Arguments @('/?') `
        -DisplayName 'MSVC linker' `
        -AcceptedExitCodes @(0, 1100)
    $approvedVersion = (Get-WindowsNativeBuildContract).LinkVersion
    if ($version -cne $approvedVersion) {
        throw "MSVC linker version must be exactly $approvedVersion; found '$version'."
    }
    return $version
}

function Clear-InheritedMsBuildEnvironment {
    foreach ($variable in @(
        'CL',
        '_CL_',
        'LINK',
        '_LINK_',
        'ForceImportBeforeCppTargets',
        'ForceImportAfterCppTargets'
    )) {
        Remove-Item -LiteralPath "Env:$variable" -ErrorAction SilentlyContinue
    }
}

function Invoke-ControlledMsBuild {
    param(
        [Parameter(Mandatory = $true)] [string] $MsBuildPath,
        [Parameter(Mandatory = $true)] [string] $SolutionPath,
        [Parameter(Mandatory = $true)] [string] $LogPath
    )

    Clear-InheritedMsBuildEnvironment
    $contract = Get-WindowsNativeBuildContract
    $arguments = @(
        $SolutionPath,
        '-noAutoResponse',
        '/nologo',
        '/m',
        '/t:Rebuild',
        "/p:Configuration=$($contract.Configuration)",
        "/p:Platform=$($contract.Platform)",
        "/p:PlatformToolset=$($contract.PlatformToolset)",
        '/p:ImportDirectoryBuildProps=false',
        '/p:ImportDirectoryBuildTargets=false',
        '/verbosity:minimal',
        '/fileLogger',
        "/fileLoggerParameters:LogFile=$LogPath;Verbosity=normal"
    )
    return Invoke-NativeCommandWithOutput -ExecutablePath $MsBuildPath -Arguments $arguments
}

function Invoke-LibsodiumArchiveInspection {
    param(
        [Parameter(Mandatory = $true)] [string] $LinkPath,
        [Parameter(Mandatory = $true)] [string] $LibraryPath
    )

    $directiveLines = @(& $LinkPath /dump /directives $LibraryPath 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "link.exe /dump /directives failed for libsodium.lib (exit $LASTEXITCODE)."
    }
    $headerLines = @(& $LinkPath /dump /headers $LibraryPath 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "link.exe /dump /headers failed for libsodium.lib (exit $LASTEXITCODE)."
    }

    $directiveOutput = $directiveLines -join [Environment]::NewLine
    $headerOutput = $headerLines -join [Environment]::NewLine
    Assert-LibsodiumArchiveInspection -DirectiveOutput $directiveOutput -HeaderOutput $headerOutput

    $linkVersionMatch = [regex]::Match($directiveOutput, '(?im)^Microsoft \(R\) COFF/PE Dumper Version (\d+(?:\.\d+)+)\s*$')
    if (-not $linkVersionMatch.Success) {
        throw 'link.exe /dump did not report a parseable version.'
    }

    return [pscustomobject]@{
        LinkVersion = $linkVersionMatch.Groups[1].Value
        DirectiveOutput = $directiveOutput
        HeaderOutput = $headerOutput
    }
}

function Remove-NativeTree {
    param(
        [Parameter(Mandatory = $true)] [string] $NativeRoot,
        [Parameter(Mandatory = $true)] [string] $Path
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }

    $nativeRootFull = [System.IO.Path]::GetFullPath($NativeRoot).TrimEnd('\', '/')
    $pathFull = [System.IO.Path]::GetFullPath($Path)
    $nativePrefix = $nativeRootFull + [System.IO.Path]::DirectorySeparatorChar
    if (-not $pathFull.StartsWith($nativePrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove a native build path outside the project cache: $pathFull"
    }

    Remove-Item -LiteralPath $pathFull -Recurse -Force
}

function Get-VerifiedLibsodiumSourceArchive {
    param(
        [Parameter(Mandatory = $true)] [string] $NativeRoot
    )

    $contract = Get-WindowsNativeBuildContract
    $archivePath = Join-Path $NativeRoot $contract.SourceAsset
    if (Test-Path -LiteralPath $archivePath -PathType Leaf) {
        $cachedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $archivePath).Hash.ToLowerInvariant()
        if ($cachedHash -cne $contract.SourceSha256) {
            Remove-Item -LiteralPath $archivePath -Force
            throw "Cached libsodium source checksum mismatch: expected $($contract.SourceSha256), got $cachedHash."
        }
        return $archivePath
    }

    $curl = Get-Command curl.exe -ErrorAction SilentlyContinue
    if ($null -eq $curl) {
        throw 'curl.exe is required to download the official libsodium source archive.'
    }

    $partialPath = "$archivePath.download.$PID.$([guid]::NewGuid().ToString('N'))"
    try {
        & $curl.Source --fail --location --retry 3 --connect-timeout 30 --max-time 300 --output $partialPath $contract.SourceUrl
        if ($LASTEXITCODE -ne 0) {
            throw "Unable to download the official libsodium source archive (curl exit $LASTEXITCODE)."
        }

        $downloadedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $partialPath).Hash.ToLowerInvariant()
        if ($downloadedHash -cne $contract.SourceSha256) {
            throw "Downloaded libsodium source checksum mismatch: expected $($contract.SourceSha256), got $downloadedHash."
        }

        Move-Item -LiteralPath $partialPath -Destination $archivePath
    }
    finally {
        Remove-Item -LiteralPath $partialPath -Force -ErrorAction SilentlyContinue
    }

    return $archivePath
}

function New-LibsodiumBuildReceipt {
    param(
        [Parameter(Mandatory = $true)] [string] $LibraryPath,
        [Parameter(Mandatory = $true)] [string] $MsBuildVersion,
        [Parameter(Mandatory = $true)] [string] $LinkVersion,
        [Parameter(Mandatory = $true)] [string] $VcToolsVersion
    )

    $contract = Get-WindowsNativeBuildContract
    $libraryHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $LibraryPath).Hash.ToLowerInvariant()
    return [ordered]@{
        schema_version = $contract.SchemaVersion
        recipe_revision = $contract.RecipeRevision
        libsodium_version = $contract.LibsodiumVersion
        source_asset = $contract.SourceAsset
        source_url = $contract.SourceUrl
        source_sha256 = $contract.SourceSha256
        solution = $contract.Solution
        configuration = $contract.Configuration
        platform = $contract.Platform
        platform_toolset = $contract.PlatformToolset
        runtime_library = $contract.RuntimeLibrary
        property_file = $contract.PropertyFile
        property_transform_count = 1
        debug_property_file = $contract.DebugPropertyFile
        debug_information_format = $contract.DebugInformationFormat
        debug_property_transform_count = 1
        library = $contract.Library
        library_sha256 = $libraryHash
        machine = 'x64'
        crt_default_library = 'MSVCRT'
        visual_studio_installation_version = $contract.VisualStudioInstallationVersion
        msbuild_version = $MsBuildVersion
        link_version = $LinkVersion
        vc_tools_version = $VcToolsVersion
    }
}

function Get-ValidatedProvisionedArtifact {
    param(
        [Parameter(Mandatory = $true)] [string] $ArtifactRoot,
        [Parameter(Mandatory = $true)] [string] $LinkPath,
        [Parameter(Mandatory = $true)] [string] $MsBuildPath,
        [Parameter(Mandatory = $true)] [string] $VcToolsVersion
    )

    $contract = Get-WindowsNativeBuildContract
    $libraryPath = Join-Path $ArtifactRoot ($contract.Library.Replace('/', '\'))
    $receiptPath = Join-Path $ArtifactRoot 'receipt.json'
    $receipt = Assert-LibsodiumBuildReceipt -ReceiptPath $receiptPath -LibraryPath $libraryPath
    $inspection = Invoke-LibsodiumArchiveInspection -LinkPath $LinkPath -LibraryPath $libraryPath
    if ($receipt.link_version -cne $inspection.LinkVersion) {
        throw 'The cached libsodium receipt was produced with a different MSVC linker version.'
    }
    $currentMsBuildVersion = Get-ExecutableVersion -ExecutablePath $MsBuildPath -Arguments @('-version', '-nologo') -DisplayName 'MSBuild'
    if ($receipt.msbuild_version -cne $currentMsBuildVersion) {
        throw 'The cached libsodium receipt was produced with a different MSBuild version.'
    }
    if ($receipt.vc_tools_version -cne $VcToolsVersion) {
        throw 'The cached libsodium receipt was produced with a different VC tools version.'
    }

    return [pscustomobject]@{
        ArtifactRoot = $ArtifactRoot
        LibraryPath = $libraryPath
        LibraryDirectory = Split-Path -Parent $libraryPath
        ReceiptPath = $receiptPath
    }
}

function Publish-RollbackSafeArtifact {
    param(
        [Parameter(Mandatory = $true)] [string] $NativeRoot,
        [Parameter(Mandatory = $true)] [string] $StagedArtifactRoot,
        [Parameter(Mandatory = $true)] [string] $ArtifactRoot,
        [Parameter(Mandatory = $true)] [scriptblock] $ValidateArtifact,
        [object[]] $ValidationArguments = @(),
        [scriptblock] $MoveDirectory = {
            param($source, $destination)
            [System.IO.Directory]::Move($source, $destination)
        },
        [scriptblock] $CleanupBackup
    )

    $backupRoot = Join-Path $NativeRoot ("backup-" + [guid]::NewGuid().ToString('N'))
    $previousMoved = $false
    $replacementMoved = $false
    try {
        if (Test-Path -LiteralPath $ArtifactRoot) {
            & $MoveDirectory $ArtifactRoot $backupRoot
            $previousMoved = $true
        }

        & $MoveDirectory $StagedArtifactRoot $ArtifactRoot
        $replacementMoved = $true
        $validatedArtifact = & $ValidateArtifact $ArtifactRoot @ValidationArguments
    }
    catch {
        $publicationError = $_.Exception
        try {
            if ($replacementMoved -and (Test-Path -LiteralPath $ArtifactRoot)) {
                Remove-NativeTree -NativeRoot $NativeRoot -Path $ArtifactRoot
            }
            if ($previousMoved -and (Test-Path -LiteralPath $backupRoot)) {
                if (Test-Path -LiteralPath $ArtifactRoot) {
                    Remove-NativeTree -NativeRoot $NativeRoot -Path $ArtifactRoot
                }
                & $MoveDirectory $backupRoot $ArtifactRoot
            }
        }
        catch {
            throw "Cache publication failed ('$($publicationError.Message)') and rollback failed: $($_.Exception.Message)"
        }
        throw $publicationError
    }

    if ($previousMoved) {
        try {
            if (Test-Path -LiteralPath $backupRoot) {
                if ($null -eq $CleanupBackup) {
                    Remove-NativeTree -NativeRoot $NativeRoot -Path $backupRoot
                }
                else {
                    & $CleanupBackup $NativeRoot $backupRoot | Out-Null
                }
            }
        }
        catch {
            Write-Warning "Validated cache publication succeeded, but backup cleanup failed: $($_.Exception.Message)"
        }
    }
    return $validatedArtifact
}

function Publish-ProvisionedLibsodiumArtifact {
    param(
        [Parameter(Mandatory = $true)] [string] $NativeRoot,
        [Parameter(Mandatory = $true)] [string] $StagedArtifactRoot,
        [Parameter(Mandatory = $true)] [string] $ArtifactRoot,
        [Parameter(Mandatory = $true)] [string] $LinkPath,
        [Parameter(Mandatory = $true)] [string] $MsBuildPath,
        [Parameter(Mandatory = $true)] [string] $VcToolsVersion
    )

    return Publish-RollbackSafeArtifact `
        -NativeRoot $NativeRoot `
        -StagedArtifactRoot $StagedArtifactRoot `
        -ArtifactRoot $ArtifactRoot `
        -ValidateArtifact {
            param($publishedRoot, $link, $msbuild, $vcTools)
            Get-ValidatedProvisionedArtifact `
                -ArtifactRoot $publishedRoot `
                -LinkPath $link `
                -MsBuildPath $msbuild `
                -VcToolsVersion $vcTools
        } `
        -ValidationArguments @($LinkPath, $MsBuildPath, $VcToolsVersion)
}

function New-ProvisionedLibsodiumArtifact {
    param(
        [Parameter(Mandatory = $true)] [string] $NativeRoot,
        [Parameter(Mandatory = $true)] [string] $ArchivePath,
        [Parameter(Mandatory = $true)] [string] $MsBuildPath,
        [Parameter(Mandatory = $true)] [string] $LinkPath,
        [Parameter(Mandatory = $true)] [string] $VcToolsVersion,
        [Parameter(Mandatory = $true)] [string] $ArtifactRoot
    )

    $contract = Get-WindowsNativeBuildContract
    $workRoot = Join-Path $NativeRoot 'w'
    $artifactStage = Join-Path $NativeRoot 's'
    try {
        Remove-NativeTree -NativeRoot $NativeRoot -Path $workRoot
        Remove-NativeTree -NativeRoot $NativeRoot -Path $artifactStage
        $extractRoot = $workRoot
        New-Item -ItemType Directory -Path $extractRoot -Force | Out-Null
        $tar = Get-Command tar.exe -ErrorAction SilentlyContinue
        if ($null -eq $tar) {
            throw 'tar.exe is required to extract the official libsodium source archive.'
        }

        & $tar.Source -xzf $ArchivePath -C $extractRoot
        if ($LASTEXITCODE -ne 0) {
            throw "The official libsodium source archive could not be extracted (tar exit $LASTEXITCODE)."
        }

        $topLevelEntries = @(Get-ChildItem -LiteralPath $extractRoot -Force)
        if ($topLevelEntries.Count -ne 1 -or
            -not $topLevelEntries[0].PSIsContainer -or
            $topLevelEntries[0].Name -cne $contract.SourceDirectory) {
            throw "The official libsodium source archive did not extract to exactly '$($contract.SourceDirectory)'."
        }

        $sourceRoot = $topLevelEntries[0].FullName
        $propertyPath = Join-Path $sourceRoot ($contract.PropertyFile.Replace('/', '\'))
        $replacementCount = Convert-LibsodiumReleasePropertyToDynamicCrt -Path $propertyPath
        if ($replacementCount -ne 1) {
            throw "The libsodium RuntimeLibrary transform count was $replacementCount instead of 1."
        }

        $debugPropertyPath = Join-Path $sourceRoot ($contract.DebugPropertyFile.Replace('/', '\'))
        $debugReplacementCount = Convert-LibsodiumReleaseDebugInformationToEmbedded -Path $debugPropertyPath
        if ($debugReplacementCount -ne 1) {
            throw "The libsodium DebugInformationFormat transform count was $debugReplacementCount instead of 1."
        }

        $solutionPath = Join-Path $sourceRoot ($contract.Solution.Replace('/', '\'))
        if (-not (Test-Path -LiteralPath $solutionPath -PathType Leaf)) {
            throw "The expected libsodium VS2022 solution is missing: $solutionPath"
        }

        $msbuildLog = Join-Path $workRoot 'msbuild.log'
        $msbuildExitCode = Invoke-ControlledMsBuild -MsBuildPath $MsBuildPath -SolutionPath $solutionPath -LogPath $msbuildLog
        if ($msbuildExitCode -ne 0) {
            $logTail = if (Test-Path -LiteralPath $msbuildLog -PathType Leaf) {
                (Get-Content -LiteralPath $msbuildLog -Tail 40) -join [Environment]::NewLine
            }
            else {
                'MSBuild did not create its requested log file.'
            }
            throw "The libsodium StaticRelease x64 build failed (MSBuild exit $msbuildExitCode).$([Environment]::NewLine)$logTail"
        }

        $builtLibrary = Join-Path $sourceRoot 'bin\x64\Release\v143\static\libsodium.lib'
        if (-not (Test-Path -LiteralPath $builtLibrary -PathType Leaf) -or (Get-Item -LiteralPath $builtLibrary).Length -eq 0) {
            throw 'MSBuild did not produce the expected nonempty x64 StaticRelease libsodium.lib.'
        }
        $builtDlls = @(Get-ChildItem -LiteralPath (Split-Path -Parent $builtLibrary) -Filter '*.dll' -File -ErrorAction SilentlyContinue)
        if ($builtDlls.Count -ne 0) {
            throw 'The static libsodium build unexpectedly produced a DLL.'
        }

        $inspection = Invoke-LibsodiumArchiveInspection -LinkPath $LinkPath -LibraryPath $builtLibrary
        $msbuildVersion = Get-ExecutableVersion -ExecutablePath $MsBuildPath -Arguments @('-version', '-nologo') -DisplayName 'MSBuild'

        $stageLibraryDirectory = Join-Path $artifactStage 'lib'
        New-Item -ItemType Directory -Path $stageLibraryDirectory -Force | Out-Null
        $stageLibrary = Join-Path $stageLibraryDirectory 'libsodium.lib'
        Copy-Item -LiteralPath $builtLibrary -Destination $stageLibrary

        $receipt = New-LibsodiumBuildReceipt -LibraryPath $stageLibrary -MsBuildVersion $msbuildVersion -LinkVersion $inspection.LinkVersion -VcToolsVersion $VcToolsVersion
        $receiptPath = Join-Path $artifactStage 'receipt.json'
        $receiptJson = $receipt | ConvertTo-Json -Depth 3
        [System.IO.File]::WriteAllText($receiptPath, $receiptJson + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))

        Assert-LibsodiumBuildReceipt -ReceiptPath $receiptPath -LibraryPath $stageLibrary | Out-Null
        Invoke-LibsodiumArchiveInspection -LinkPath $LinkPath -LibraryPath $stageLibrary | Out-Null

        return Publish-ProvisionedLibsodiumArtifact `
            -NativeRoot $NativeRoot `
            -StagedArtifactRoot $artifactStage `
            -ArtifactRoot $ArtifactRoot `
            -LinkPath $LinkPath `
            -MsBuildPath $MsBuildPath `
            -VcToolsVersion $VcToolsVersion
    }
    finally {
        Remove-NativeTree -NativeRoot $NativeRoot -Path $workRoot
        Remove-NativeTree -NativeRoot $NativeRoot -Path $artifactStage
    }
}

function Get-ProvisionedLibsodiumArtifact {
    param(
        [Parameter(Mandatory = $true)] [string] $NativeRoot,
        [Parameter(Mandatory = $true)] [string] $MsBuildPath,
        [Parameter(Mandatory = $true)] [string] $LinkPath,
        [Parameter(Mandatory = $true)] [string] $VcToolsVersion
    )

    $contract = Get-WindowsNativeBuildContract
    $artifactRoot = Join-Path $NativeRoot $contract.CacheDirectory
    if (Test-Path -LiteralPath $artifactRoot) {
        try {
            return Get-ValidatedProvisionedArtifact -ArtifactRoot $artifactRoot -LinkPath $LinkPath -MsBuildPath $MsBuildPath -VcToolsVersion $VcToolsVersion
        }
        catch {
            Write-Warning "Rejecting cached libsodium artifact: $($_.Exception.Message)"
        }
    }

    $archivePath = Get-VerifiedLibsodiumSourceArchive -NativeRoot $NativeRoot
    return New-ProvisionedLibsodiumArtifact -NativeRoot $NativeRoot -ArchivePath $archivePath -MsBuildPath $MsBuildPath -LinkPath $LinkPath -VcToolsVersion $VcToolsVersion -ArtifactRoot $artifactRoot
}

function Get-CargoTargetDirectoryArgument {
    param(
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $found = $false
    $targetDirectory = $null
    for ($index = 0; $index -lt $CargoArguments.Count; $index++) {
        if ($CargoArguments[$index] -eq '--') {
            break
        }
        if ($CargoArguments[$index] -eq '--target-dir') {
            if ($index + 1 -ge $CargoArguments.Count -or $CargoArguments[$index + 1] -eq '--') {
                throw 'Cargo --target-dir is missing its path argument.'
            }
            if ($found) {
                throw 'Cargo --target-dir may be supplied only once.'
            }
            $targetDirectory = $CargoArguments[$index + 1]
            $found = $true
            $index++
            continue
        }
        if ($CargoArguments[$index].StartsWith('--target-dir=', [System.StringComparison]::Ordinal)) {
            if ($found) {
                throw 'Cargo --target-dir may be supplied only once.'
            }
            $targetDirectory = $CargoArguments[$index].Substring('--target-dir='.Length)
            if ([string]::IsNullOrWhiteSpace($targetDirectory)) {
                throw 'Cargo --target-dir is missing its path argument.'
            }
            $found = $true
        }
    }
    return $targetDirectory
}

function Get-CargoTargetArgument {
    param(
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $found = $false
    $target = $null
    for ($index = 0; $index -lt $CargoArguments.Count; $index++) {
        $argument = $CargoArguments[$index]
        if ($argument -eq '--') {
            break
        }
        if ($argument -eq '--target') {
            if ($index + 1 -ge $CargoArguments.Count -or $CargoArguments[$index + 1] -eq '--') {
                throw 'Cargo --target requires a target triple.'
            }
            if ($found) {
                throw 'Cargo --target may be supplied only once.'
            }
            $target = $CargoArguments[$index + 1]
            $found = $true
            $index++
            continue
        }
        if ($argument.StartsWith('--target=', [System.StringComparison]::Ordinal)) {
            if ($found) {
                throw 'Cargo --target may be supplied only once.'
            }
            $target = $argument.Substring('--target='.Length)
            if ([string]::IsNullOrWhiteSpace($target)) {
                throw 'Cargo --target requires a target triple.'
            }
            $found = $true
        }
    }
    return $target
}

function Get-CargoProfileDirectory {
    param(
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $profile = 'debug'
    for ($index = 0; $index -lt $CargoArguments.Count; $index++) {
        $argument = $CargoArguments[$index]
        if ($argument -eq '--') {
            break
        }
        if ($argument -eq '--release' -or $argument -eq '-r') {
            $profile = 'release'
            continue
        }
        if ($argument -eq '--profile') {
            if ($index + 1 -ge $CargoArguments.Count -or $CargoArguments[$index + 1] -eq '--') {
                throw 'Cargo --profile requires a profile name.'
            }
            $profile = $CargoArguments[$index + 1]
            $index++
            continue
        }
        if ($argument.StartsWith('--profile=', [System.StringComparison]::Ordinal)) {
            $profile = $argument.Substring('--profile='.Length)
            if ([string]::IsNullOrWhiteSpace($profile)) {
                throw 'Cargo --profile requires a profile name.'
            }
        }
    }
    if ($profile -eq 'dev' -or $profile -eq 'test') {
        return 'debug'
    }
    return $profile
}

function Get-EffectiveCargoTarget {
    param(
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $contract = Get-WindowsNativeBuildContract
    $requestedTarget = Get-CargoTargetArgument -CargoArguments $CargoArguments
    if ($requestedTarget -and $requestedTarget -cne $contract.CargoTarget) {
        throw "Unsupported Cargo target '$requestedTarget'; only $($contract.CargoTarget) is allowed."
    }
    return $contract.CargoTarget
}

function Get-CargoBuildRoot {
    param(
        [Parameter(Mandatory = $true)] [string] $TargetRoot,
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $root = $TargetRoot
    $effectiveTarget = Get-EffectiveCargoTarget -CargoArguments $CargoArguments
    if ($effectiveTarget) {
        $root = Join-Path $root $effectiveTarget
    }
    return Join-Path $root (Get-CargoProfileDirectory -CargoArguments $CargoArguments)
}

function Assert-CargoTargetPathBudget {
    param(
        [Parameter(Mandatory = $true)] [string] $TargetRoot,
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $buildRoot = Get-CargoBuildRoot -TargetRoot $TargetRoot -CargoArguments $CargoArguments
    $openSslObjectSuffix = 'build\openssl-sys-0123456789abcdef\out\openssl-build\build\src\providers\implementations\ciphers\libdefault-lib-cipher_aes_cbc_hmac_sha256_etm_hw.obj'
    $probePath = Join-Path $buildRoot $openSslObjectSuffix
    if ($probePath.Length -ge 260) {
        throw "Cargo target directory is too long for vendored OpenSSL 3.6.3: the representative object path is $($probePath.Length) characters (must be less than 260). Supply a shorter absolute --target-dir. Probe: $probePath"
    }
}

function Get-CargoTargetRoot {
    param(
        [Parameter(Mandatory = $true)] [string] $ManifestRoot,
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $configuredTarget = Get-CargoTargetDirectoryArgument -CargoArguments $CargoArguments
    if (-not $configuredTarget) {
        return Join-Path $ManifestRoot '.native\t'
    }
    if ([System.IO.Path]::IsPathRooted($configuredTarget)) {
        return [System.IO.Path]::GetFullPath($configuredTarget)
    }
    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $configuredTarget))
}

function Add-CargoArgumentsBeforeApplicationBoundary {
    param(
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments,
        [Parameter(Mandatory = $true)] [string[]] $ArgumentsToAdd
    )

    $boundary = [array]::IndexOf($CargoArguments, '--')
    if ($boundary -lt 0) {
        $result = @($CargoArguments) + @($ArgumentsToAdd)
        return $result
    }

    $beforeBoundary = if ($boundary -eq 0) { @() } else { @($CargoArguments[0..($boundary - 1)]) }
    $fromBoundary = @($CargoArguments[$boundary..($CargoArguments.Count - 1)])
    $result = @($beforeBoundary) + @($ArgumentsToAdd) + @($fromBoundary)
    return $result
}

function ConvertTo-CargoTomlBasicString {
    param(
        [Parameter(Mandatory = $true)] [AllowEmptyString()] [string] $Value
    )

    if ($Value -match '[\x00-\x1f]') {
        throw 'A controlled Cargo environment value contains an unsupported control character.'
    }
    return '"' + $Value.Replace('\', '\\').Replace('"', '\"') + '"'
}

function Initialize-ControlledCargoConfiguration {
    param(
        [Parameter(Mandatory = $true)] [string] $ManifestRoot,
        [Parameter(Mandatory = $true)] [string] $OpenSslConfigurationDirectory
    )

    $nativeRoot = Join-Path $ManifestRoot '.native'
    New-Item -ItemType Directory -Path $nativeRoot -Force | Out-Null
    $configurationPath = Join-Path $nativeRoot 'windows-cargo-config.toml'
    $openSslConfigurationValue = ConvertTo-CargoTomlBasicString -Value $OpenSslConfigurationDirectory
    $content = @(
        '[env]'
        'CFLAGS = { value = "", force = true }'
        'CXXFLAGS = { value = "", force = true }'
        'OPENSSL_NO_VENDOR = { value = "0", force = true }'
        'X86_64_PC_WINDOWS_MSVC_OPENSSL_NO_VENDOR = { value = "0", force = true }'
        "OPENSSL_CONFIG_DIR = { value = $openSslConfigurationValue, force = true }"
        "X86_64_PC_WINDOWS_MSVC_OPENSSL_CONFIG_DIR = { value = $openSslConfigurationValue, force = true }"
        "OPENSSL_LOCAL_CONFIG_DIR = { value = $openSslConfigurationValue, force = true }"
    ) -join [Environment]::NewLine
    $content += [Environment]::NewLine

    $temporaryPath = "$configurationPath.tmp.$([guid]::NewGuid().ToString('N'))"
    try {
        [System.IO.File]::WriteAllText(
            $temporaryPath,
            $content,
            [System.Text.UTF8Encoding]::new($false)
        )
        Move-Item -LiteralPath $temporaryPath -Destination $configurationPath -Force
    }
    finally {
        Remove-Item -LiteralPath $temporaryPath -Force -ErrorAction SilentlyContinue
    }
    if ([System.IO.File]::ReadAllText($configurationPath) -cne $content) {
        throw 'The controlled Cargo configuration was not published exactly.'
    }
    return $configurationPath
}

function Add-CargoLockedArgument {
    param(
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $boundary = [array]::IndexOf($CargoArguments, '--')
    $commandArguments = if ($boundary -lt 0) {
        @($CargoArguments)
    }
    elseif ($boundary -eq 0) {
        @()
    }
    else {
        @($CargoArguments[0..($boundary - 1)])
    }
    $lockedCount = @($commandArguments | Where-Object { $_ -ceq '--locked' }).Count
    if ($lockedCount -gt 1) {
        throw 'Cargo --locked may be supplied only once.'
    }
    if ($lockedCount -eq 1) {
        return @($CargoArguments)
    }
    return @(Add-CargoArgumentsBeforeApplicationBoundary `
            -CargoArguments $CargoArguments `
            -ArgumentsToAdd @('--locked'))
}

function Get-CargoSubcommand {
    param(
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $boundary = [array]::IndexOf($CargoArguments, '--')
    $commandArgumentCount = if ($boundary -lt 0) { $CargoArguments.Count } else { $boundary }
    $globalOptionsWithValues = @('--color', '--config', '-C', '-Z')
    for ($index = 0; $index -lt $commandArgumentCount; $index++) {
        $argument = $CargoArguments[$index]
        if ($index -eq 0 -and $argument.StartsWith('+', [System.StringComparison]::Ordinal)) {
            continue
        }
        if ($globalOptionsWithValues -ccontains $argument) {
            $index++
            continue
        }
        if ($argument.StartsWith('-', [System.StringComparison]::Ordinal)) {
            continue
        }
        return $argument
    }
    return $null
}

function Assert-SafeCargoArguments {
    param(
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $boundary = [array]::IndexOf($CargoArguments, '--')
    $commandArguments = @(if ($boundary -lt 0) {
            @($CargoArguments)
        }
        elseif ($boundary -eq 0) {
            @()
        }
        else {
            @($CargoArguments[0..($boundary - 1)])
        })

    $subcommand = Get-CargoSubcommand -CargoArguments $CargoArguments
    if ($subcommand -ceq 'rustc') {
        throw 'cargo rustc is not supported by the Windows native wrapper.'
    }
    if ($subcommand -ceq 'bench') {
        throw 'cargo bench is not supported by the Windows native wrapper.'
    }

    for ($index = 0; $index -lt $commandArguments.Count; $index++) {
        $argument = $commandArguments[$index]
        if ($argument -ceq '--config' -or $argument.StartsWith('--config=', [System.StringComparison]::Ordinal)) {
            throw 'Cargo --config may not override the Windows native build contract.'
        }

        $codegen = $null
        if ($argument -ceq '-C' -or $argument -ceq '--codegen') {
            if ($index + 1 -lt $commandArguments.Count) {
                $index++
                $codegen = $commandArguments[$index]
            }
        }
        elseif ($argument.StartsWith('-C', [System.StringComparison]::Ordinal) -and $argument.Length -gt 2) {
            $codegen = $argument.Substring(2)
        }
        elseif ($argument.StartsWith('--codegen=', [System.StringComparison]::Ordinal)) {
            $codegen = $argument.Substring('--codegen='.Length)
        }

        if ($codegen -match '(?i)^linker=') {
            throw 'Cargo arguments may not override the selected linker.'
        }
        if ($codegen -match '(?i)^target-feature=(?:[^,]+,)*\+crt-static(?:,|$)') {
            throw 'Cargo arguments may not enable crt-static.'
        }
    }
}

function New-ControlledCargoInvocation {
    param(
        [Parameter(Mandatory = $true)] [string] $ManifestRoot,
        [Parameter(Mandatory = $true)] [string] $LinkPath,
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    Assert-SafeCargoArguments -CargoArguments $CargoArguments
    $contract = Get-WindowsNativeBuildContract
    $target = Get-EffectiveCargoTarget -CargoArguments $CargoArguments
    $targetRoot = Get-CargoTargetRoot -ManifestRoot $ManifestRoot -CargoArguments $CargoArguments
    $controlledArguments = [string[]] @($CargoArguments)
    if (-not (Get-CargoTargetArgument -CargoArguments $controlledArguments)) {
        $controlledArguments = [string[]] @(Add-CargoArgumentsBeforeApplicationBoundary `
                -CargoArguments $controlledArguments `
                -ArgumentsToAdd @('--target', $target))
    }
    if (-not (Get-CargoTargetDirectoryArgument -CargoArguments $controlledArguments)) {
        $controlledArguments = [string[]] @(Add-CargoArgumentsBeforeApplicationBoundary `
                -CargoArguments $controlledArguments `
                -ArgumentsToAdd @('--target-dir', $targetRoot))
    }
    $controlledArguments = [string[]] @(Add-CargoLockedArgument -CargoArguments $controlledArguments)

    foreach ($variable in @(
        'RUSTFLAGS',
        'CARGO_BUILD_RUSTFLAGS',
        'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS',
        'CFLAGS',
        'CXXFLAGS',
        'OPENSSL_NO_VENDOR',
        'X86_64_PC_WINDOWS_MSVC_OPENSSL_NO_VENDOR',
        'OPENSSL_CONFIG_DIR',
        'X86_64_PC_WINDOWS_MSVC_OPENSSL_CONFIG_DIR',
        'OPENSSL_LOCAL_CONFIG_DIR'
    )) {
        Remove-Item -LiteralPath "Env:$variable" -ErrorAction SilentlyContinue
    }
    $env:CARGO_BUILD_TARGET = $target
    $env:CARGO_TARGET_DIR = $targetRoot
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER = $LinkPath
    $env:CARGO_ENCODED_RUSTFLAGS = '-Ctarget-feature=-crt-static'
    $templateRevision = $contract.OpenSslPatchedWindowsMakefileSha256.Substring(0, 12)
    $openSslConfigurationDirectory = Join-Path $ManifestRoot ".native\openssl-config-$($contract.OpenSslVersion)-$templateRevision"
    $env:OPENSSL_CONFIG_DIR = $openSslConfigurationDirectory
    $env:OPENSSL_LOCAL_CONFIG_DIR = $openSslConfigurationDirectory
    $controlledCargoConfiguration = Initialize-ControlledCargoConfiguration `
        -ManifestRoot $ManifestRoot `
        -OpenSslConfigurationDirectory $openSslConfigurationDirectory
    $controlledArguments = [string[]] @(Add-CargoArgumentsBeforeApplicationBoundary `
            -CargoArguments $controlledArguments `
            -ArgumentsToAdd @('--config', $controlledCargoConfiguration))

    return [pscustomobject]@{
        CargoArguments = $controlledArguments
        Target = $target
        TargetRoot = $targetRoot
        ProfileDirectory = Get-CargoProfileDirectory -CargoArguments $controlledArguments
        BuildRoot = Get-CargoBuildRoot -TargetRoot $targetRoot -CargoArguments $controlledArguments
    }
}

function Assert-CargoUsedProvisionedLibsodium {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)] [string] $BuildRoot,
        [Parameter(Mandatory = $true)] [string] $LibraryDirectory
    )

    if (-not (Test-Path -LiteralPath $BuildRoot -PathType Container)) {
        throw 'Cargo did not create the expected target/profile directory.'
    }

    $outputFiles = @(Get-ChildItem -Path (Join-Path $BuildRoot 'build\libsodium-sys-stable-*\output') -File -ErrorAction SilentlyContinue)
    if ($outputFiles.Count -eq 0) {
        throw 'Cargo did not produce a libsodium-sys-stable build-script output in the requested target/profile.'
    }

    $expectedDirectory = [System.IO.Path]::GetFullPath($LibraryDirectory).TrimEnd('\', '/')
    $linkSearchCount = 0
    foreach ($outputFile in $outputFiles) {
        foreach ($line in Get-Content -LiteralPath $outputFile.FullName) {
            $match = [regex]::Match($line, '^cargo(?:::|:)rustc-link-search=native=(.+)$')
            if (-not $match.Success) {
                continue
            }
            $linkSearchCount++
            $actualDirectory = [System.IO.Path]::GetFullPath($match.Groups[1].Value).TrimEnd('\', '/')
            if (-not $actualDirectory.Equals($expectedDirectory, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw "Cargo did not select the provisioned libsodium directory: $actualDirectory"
            }
        }
    }
    if ($linkSearchCount -eq 0) {
        throw 'Cargo produced no inspectable libsodium native link-search directive.'
    }
}

function Assert-CargoLibsodiumOutputAbsent {
    param(
        [Parameter(Mandatory = $true)] [string] $BuildRoot
    )

    $outputs = @(Get-ChildItem -Path (Join-Path $BuildRoot 'build\libsodium-sys-stable-*\output') -File -ErrorAction SilentlyContinue)
    if ($outputs.Count -ne 0) {
        throw 'cargo clean left a libsodium-sys-stable build-script output in the requested target/profile.'
    }
}

function Invoke-NativeCommandWithOutput {
    param(
        [Parameter(Mandatory = $true)] [string] $ExecutablePath,
        [Parameter(Mandatory = $true)] [string[]] $Arguments
    )

    $previousErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        & $ExecutablePath @Arguments | Out-Host
        return $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
}

function Get-CargoCleanProfileArguments {
    param(
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $profileArguments = @()
    for ($index = 0; $index -lt $CargoArguments.Count; $index++) {
        $argument = $CargoArguments[$index]
        if ($argument -eq '--') {
            break
        }
        if ($argument -eq '--release' -or $argument -eq '-r') {
            $profileArguments += '--release'
            continue
        }
        if ($argument -eq '--profile') {
            if ($index + 1 -ge $CargoArguments.Count -or $CargoArguments[$index + 1] -eq '--') {
                throw 'Cargo --profile requires a profile name.'
            }
            $profileArguments += @('--profile', $CargoArguments[$index + 1])
            $index++
            continue
        }
        if ($argument.StartsWith('--profile=', [System.StringComparison]::Ordinal)) {
            $profileName = $argument.Substring('--profile='.Length)
            if ([string]::IsNullOrWhiteSpace($profileName)) {
                throw 'Cargo --profile requires a profile name.'
            }
            $profileArguments += @('--profile', $profileName)
        }
    }
    return $profileArguments
}

function Get-LibsodiumCargoCleanArguments {
    param(
        [Parameter(Mandatory = $true)] [string] $ManifestRoot,
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $targetRoot = Get-CargoTargetRoot -ManifestRoot $ManifestRoot -CargoArguments $CargoArguments
    $target = Get-EffectiveCargoTarget -CargoArguments $CargoArguments
    $cleanArguments = @(
        'clean',
        '--manifest-path',
        (Join-Path $ManifestRoot 'Cargo.toml'),
        '-p',
        'libsodium-sys-stable',
        '--target-dir',
        $targetRoot,
        '--target',
        $target
    )
    $cleanArguments += @(Get-CargoCleanProfileArguments -CargoArguments $CargoArguments)
    return $cleanArguments
}

function Invoke-LibsodiumCargoClean {
    param(
        [Parameter(Mandatory = $true)] [string] $CargoPath,
        [Parameter(Mandatory = $true)] [string] $ManifestRoot,
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $cleanArguments = [string[]] @(Get-LibsodiumCargoCleanArguments `
            -ManifestRoot $ManifestRoot `
            -CargoArguments $CargoArguments)

    $cleanExitCode = Invoke-NativeCommandWithOutput -ExecutablePath $CargoPath -Arguments $cleanArguments
    if ($cleanExitCode -ne 0) {
        throw "Unable to clear cached libsodium-sys-stable artifacts (cargo clean exit $cleanExitCode)."
    }
}

function Invoke-ControlledCargoBuild {
    param(
        [Parameter(Mandatory = $true)] [string] $CargoPath,
        [Parameter(Mandatory = $true)] [string] $ManifestRoot,
        [Parameter(Mandatory = $true)] [string] $LinkPath,
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments,
        [Parameter(Mandatory = $true)] [string] $LibraryDirectory
    )

    $invocation = New-ControlledCargoInvocation `
        -ManifestRoot $ManifestRoot `
        -LinkPath $LinkPath `
        -CargoArguments $CargoArguments
    Assert-CargoTargetPathBudget `
        -TargetRoot $invocation.TargetRoot `
        -CargoArguments $invocation.CargoArguments
    Invoke-LibsodiumCargoClean `
        -CargoPath $CargoPath `
        -ManifestRoot $ManifestRoot `
        -CargoArguments $invocation.CargoArguments
    Assert-CargoLibsodiumOutputAbsent -BuildRoot $invocation.BuildRoot

    $cargoExitCode = Invoke-NativeCommandWithOutput `
        -ExecutablePath $CargoPath `
        -Arguments $invocation.CargoArguments
    if ($cargoExitCode -eq 0) {
        Assert-CargoUsedProvisionedLibsodium `
            -BuildRoot $invocation.BuildRoot `
            -LibraryDirectory $LibraryDirectory
    }
    return $cargoExitCode
}

function Assert-PortableOpenSslBuildTools {
    if ($env:PERL -and (Test-Path -LiteralPath $env:PERL -PathType Leaf)) {
        $perlDirectory = Split-Path -Parent $env:PERL
        $pathEntries = $env:PATH -split ';'
        if ($pathEntries -notcontains $perlDirectory) {
            $env:PATH = $perlDirectory + ';' + $env:PATH
        }
    }

    $perl = Get-Command perl.exe -ErrorAction SilentlyContinue
    if ($null -eq $perl) {
        throw 'Portable Strawberry Perl is required on PATH for the vendored OpenSSL build.'
    }
    & $perl.Source -MLocale::Maketext::Simple -e 1
    if ($LASTEXITCODE -ne 0) {
        throw 'The selected Perl lacks Locale::Maketext::Simple; use portable Strawberry Perl.'
    }

    $nasm = Get-Command nasm.exe -ErrorAction SilentlyContinue
    if ($null -eq $nasm) {
        throw 'Portable NASM is required on PATH for the vendored OpenSSL build.'
    }
    & $nasm.Source -v | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "The selected NASM executable failed its version check (exit $LASTEXITCODE)."
    }
}

function Invoke-WindowsNativeBuild {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [string] $ManifestRoot,

        [switch] $ProvisionOnly,

        [string[]] $CargoArguments = @()
    )

    $manifestRootFull = [System.IO.Path]::GetFullPath($ManifestRoot)
    $nativeRoot = Join-Path $manifestRootFull '.native'
    New-Item -ItemType Directory -Path $nativeRoot -Force | Out-Null

    $lockPath = Join-Path $nativeRoot 'windows-native-build.lock'
    try {
        $lock = [System.IO.File]::Open(
            $lockPath,
            [System.IO.FileMode]::OpenOrCreate,
            [System.IO.FileAccess]::ReadWrite,
            [System.IO.FileShare]::None
        )
    }
    catch {
        throw 'Another Windows native provision/build command is already using the project-local cache.'
    }

    try {
        $vswhere = Get-VsWherePath
        $installation = Get-VisualStudioInstallationPath -VsWherePath $vswhere
        $toolchain = Get-VisualStudio2022Toolchain -InstallationPath $installation
        Initialize-VisualStudioBuildEnvironment -DeveloperCommand $toolchain.DeveloperCommand
        Assert-SelectedVisualStudioEnvironment -Toolchain $toolchain
        $vcToolsVersion = Split-Path -Leaf $toolchain.VcToolsInstallDirectory.TrimEnd('\')
        $msBuildVersion = Get-ExecutableVersion -ExecutablePath $toolchain.MsBuildPath -Arguments @('-version', '-nologo') -DisplayName 'MSBuild'
        $linkVersion = Get-ApprovedMsvcLinkerVersion -LinkPath $toolchain.LinkPath
        $contract = Get-WindowsNativeBuildContract
        Assert-ApprovedToolchainVersions `
            -VisualStudioInstallationVersion $contract.VisualStudioInstallationVersion `
            -MsBuildVersion $msBuildVersion `
            -VcToolsVersion $vcToolsVersion `
            -LinkVersion $linkVersion
        Clear-InheritedMsBuildEnvironment

        $artifact = Get-ProvisionedLibsodiumArtifact -NativeRoot $nativeRoot -MsBuildPath $toolchain.MsBuildPath -LinkPath $toolchain.LinkPath -VcToolsVersion $vcToolsVersion
        Write-Host "Verified libsodium $((Get-WindowsNativeBuildContract).LibsodiumVersion) StaticRelease x64 /MD artifact."

        if ($ProvisionOnly) {
            return 0
        }

        Assert-PortableOpenSslBuildTools
        $cargo = Get-Command cargo.exe -ErrorAction SilentlyContinue
        if ($null -eq $cargo) {
            throw 'cargo.exe was not found on PATH.'
        }

        $openSslConfigurationDirectory = Initialize-OpenSslLocalConfiguration `
            -CargoPath $cargo.Source `
            -ManifestRoot $manifestRootFull
        Write-Host "Prepared pinned OpenSSL $((Get-WindowsNativeBuildContract).OpenSslVersion) embedded-debug configuration at $openSslConfigurationDirectory."

        $env:SODIUM_LIB_DIR = $artifact.LibraryDirectory
        $env:DESKTOP_LIBSODIUM_RECEIPT = $artifact.ReceiptPath
        $env:DESKTOP_MSVC_LINK = $toolchain.LinkPath
        $env:DESKTOP_MSBUILD_VERSION = $msBuildVersion
        $env:DESKTOP_VC_TOOLS_VERSION = $vcToolsVersion
        Remove-Item Env:SODIUM_SHARED, Env:SODIUM_USE_PKG_CONFIG, Env:VCPKG_ROOT, Env:VCPKGRS_DYNAMIC, Env:VCPKGRS_TRIPLET -ErrorAction SilentlyContinue

        if ($CargoArguments.Count -eq 0) {
            $CargoArguments = @(
                'test',
                '--manifest-path',
                (Join-Path $manifestRootFull 'Cargo.toml'),
                '--test',
                'storage',
                '--',
                '--test-threads=1'
            )
        }

        return Invoke-ControlledCargoBuild `
            -CargoPath $cargo.Source `
            -ManifestRoot $manifestRootFull `
            -LinkPath $toolchain.LinkPath `
            -CargoArguments $CargoArguments `
            -LibraryDirectory $artifact.LibraryDirectory
    }
    finally {
        $lock.Dispose()
    }
}

Export-ModuleMember -Function @(
    'Assert-CargoUsedProvisionedLibsodium',
    'Assert-LibsodiumArchiveInspection',
    'Assert-LibsodiumBuildReceipt',
    'Convert-LibsodiumReleaseDebugInformationToEmbedded',
    'Convert-LibsodiumReleasePropertyToDynamicCrt',
    'Get-VisualStudio2022Toolchain',
    'Get-WindowsNativeBuildContract',
    'Invoke-WindowsNativeBuild'
)
