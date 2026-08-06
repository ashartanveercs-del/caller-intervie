param(
    [string] $TestName
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$scriptsRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$modulePath = Join-Path $scriptsRoot 'windows-native-build.psm1'

Import-Module $modulePath -Force

$script:passed = 0

function Assert-Equal {
    param(
        [Parameter(Mandatory = $true)] $Expected,
        [Parameter(Mandatory = $true)] $Actual,
        [Parameter(Mandatory = $true)] [string] $Message
    )

    if ($Expected -ne $Actual) {
        throw "$Message Expected '$Expected', got '$Actual'."
    }
}

function Assert-Throws {
    param(
        [Parameter(Mandatory = $true)] [scriptblock] $Action,
        [Parameter(Mandatory = $true)] [string] $MessagePattern
    )

    $thrown = $false
    try {
        & $Action
    }
    catch {
        $thrown = $true
        if ($_.Exception.Message -notmatch $MessagePattern) {
            throw "Expected error matching '$MessagePattern', got '$($_.Exception.Message)'."
        }
    }

    if (-not $thrown) {
        throw "Expected an error matching '$MessagePattern', but no error was raised."
    }
}

function Invoke-Test {
    param(
        [Parameter(Mandatory = $true)] [string] $Name,
        [Parameter(Mandatory = $true)] [scriptblock] $Test
    )

    if ($TestName -and $Name -notlike $TestName) {
        return
    }

    & $Test
    $script:passed++
    Write-Host "PASS: $Name"
}

function New-TestReceipt {
    param(
        [Parameter(Mandatory = $true)] [string] $LibraryPath,
        [hashtable] $Overrides = @{},
        [string[]] $Omit = @(),
        [hashtable] $Extra = @{}
    )

    $contract = Get-WindowsNativeBuildContract
    $libraryHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $LibraryPath).Hash.ToLowerInvariant()
    $receipt = [ordered]@{
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
        msbuild_version = $contract.MsBuildVersion
        vc_tools_version = $contract.VcToolsVersion
        link_version = $contract.LinkVersion
    }
    foreach ($entry in $Overrides.GetEnumerator()) {
        $receipt[$entry.Key] = $entry.Value
    }
    foreach ($name in $Omit) {
        $receipt.Remove($name)
    }
    foreach ($entry in $Extra.GetEnumerator()) {
        $receipt[$entry.Key] = $entry.Value
    }
    return $receipt
}

$script:openSslObjectProbeSuffix = 'build\openssl-sys-0123456789abcdef\out\openssl-build\build\src\providers\implementations\ciphers\libdefault-lib-cipher_aes_cbc_hmac_sha256_etm_hw.obj'

function New-TestCargoTargetRootForProbeLength {
    param(
        [Parameter(Mandatory = $true)] [int] $ProbeLength,
        [string] $Profile = 'release'
    )

    $targetAndProfileSuffix = "x86_64-pc-windows-msvc\$Profile\$script:openSslObjectProbeSuffix"
    $targetRootLength = $ProbeLength - 1 - $targetAndProfileSuffix.Length
    if ($targetRootLength -lt 3) {
        throw "Probe length $ProbeLength is too short for an absolute Windows target root."
    }
    return 'C:\' + ('t' * ($targetRootLength - 3))
}

$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("windows-native-build-tests-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null

try {
    Invoke-Test 'the ReleaseLIB property transform changes exactly one MT node to MD' {
        $propertyPath = Join-Path $testRoot 'ReleaseLIB.props'
        $source = @'
<?xml version="1.0" encoding="utf-8"?>
<Project xmlns="http://schemas.microsoft.com/developer/msbuild/2003">
  <ItemDefinitionGroup>
    <ClCompile>
      <RuntimeLibrary>MultiThreaded</RuntimeLibrary>
    </ClCompile>
  </ItemDefinitionGroup>
</Project>
'@
        [System.IO.File]::WriteAllText($propertyPath, $source, [System.Text.UTF8Encoding]::new($true))

        $count = Convert-LibsodiumReleasePropertyToDynamicCrt -Path $propertyPath
        $transformed = [System.IO.File]::ReadAllText($propertyPath)

        Assert-Equal 1 $count 'Unexpected replacement count.'
        Assert-Equal 0 ([regex]::Matches($transformed, '<RuntimeLibrary>MultiThreaded</RuntimeLibrary>').Count) 'The static CRT node remains.'
        Assert-Equal 1 ([regex]::Matches($transformed, '<RuntimeLibrary>MultiThreadedDLL</RuntimeLibrary>').Count) 'The dynamic CRT node is missing.'
    }

    Invoke-Test 'the ReleaseLIB property transform rejects source drift' {
        $propertyPath = Join-Path $testRoot 'ReleaseLIB-drifted.props'
        [System.IO.File]::WriteAllText($propertyPath, '<Project />')

        Assert-Throws {
            Convert-LibsodiumReleasePropertyToDynamicCrt -Path $propertyPath
        } 'exactly one.*MultiThreaded'
    }

    Invoke-Test 'the ReleaseLIB property transform rejects multiple MT nodes' {
        $propertyPath = Join-Path $testRoot 'ReleaseLIB-duplicated.props'
        $node = '<RuntimeLibrary>MultiThreaded</RuntimeLibrary>'
        [System.IO.File]::WriteAllText($propertyPath, "<Project>$node$node</Project>")

        Assert-Throws {
            Convert-LibsodiumReleasePropertyToDynamicCrt -Path $propertyPath
        } 'exactly one.*MultiThreaded'
    }

    Invoke-Test 'directive inspection accepts an x64 dynamic-CRT static archive' {
        $directives = @'
   /DEFAULTLIB:"MSVCRT" /DEFAULTLIB:"OLDNAMES"
'@
        $headers = '            8664 machine (x64)'

        Assert-LibsodiumArchiveInspection -DirectiveOutput $directives -HeaderOutput $headers
    }

    Invoke-Test 'directive inspection accepts the optional approved RuntimeLibrary mismatch evidence' {
        $directives = '   /DEFAULTLIB:"MSVCRT" /FAILIFMISMATCH:"RuntimeLibrary=MD_DynamicRelease"'
        $headers = '            8664 machine (x64)'

        Assert-LibsodiumArchiveInspection -DirectiveOutput $directives -HeaderOutput $headers
    }

    Invoke-Test 'directive inspection rejects LIBCMT even when MSVCRT is also present' {
        $directives = @'
   /DEFAULTLIB:"MSVCRT" /DEFAULTLIB:"LIBCMT"
'@
        $headers = '            8664 machine (x64)'

        Assert-Throws {
            Assert-LibsodiumArchiveInspection -DirectiveOutput $directives -HeaderOutput $headers
        } 'LIBCMT'
    }

    Invoke-Test 'directive inspection rejects the debug dynamic CRT' {
        $directives = @'
   /DEFAULTLIB:"MSVCRT" /DEFAULTLIB:"MSVCRTD"
'@
        $headers = '            8664 machine (x64)'

        Assert-Throws {
            Assert-LibsodiumArchiveInspection -DirectiveOutput $directives -HeaderOutput $headers
        } 'MSVCRTD'
    }

    Invoke-Test 'directive inspection rejects LIBCMTD and incompatible RuntimeLibrary evidence' {
        $headers = '            8664 machine (x64)'
        foreach ($case in @(
            @{ Directives = ' /DEFAULTLIB:"MSVCRT" /DEFAULTLIB:"LIBCMTD"'; Pattern = 'LIBCMT' },
            @{ Directives = ' /DEFAULTLIB:"MSVCRT" /FAILIFMISMATCH:"RuntimeLibrary=MT_StaticRelease"'; Pattern = 'RuntimeLibrary' }
        )) {
            Assert-Throws {
                Assert-LibsodiumArchiveInspection -DirectiveOutput $case.Directives -HeaderOutput $headers
            } $case.Pattern
        }
    }

    Invoke-Test 'directive inspection rejects an archive without the release dynamic CRT' {
        $directives = '   /DEFAULTLIB:"OLDNAMES"'
        $headers = '            8664 machine (x64)'

        Assert-Throws {
            Assert-LibsodiumArchiveInspection -DirectiveOutput $directives -HeaderOutput $headers
        } 'MSVCRT'
    }

    Invoke-Test 'directive inspection rejects non-x64 objects' {
        $directives = '   /DEFAULTLIB:"MSVCRT"'
        $headers = '             14C machine (x86)'

        Assert-Throws {
            Assert-LibsodiumArchiveInspection -DirectiveOutput $directives -HeaderOutput $headers
        } 'x64'
    }

    Invoke-Test 'receipt validation binds the contract to the library hash' {
        $contract = Get-WindowsNativeBuildContract
        $libraryPath = Join-Path $testRoot 'libsodium.lib'
        $receiptPath = Join-Path $testRoot 'receipt.json'
        [System.IO.File]::WriteAllBytes($libraryPath, [byte[]] (1, 2, 3, 4))
        $libraryHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $libraryPath).Hash.ToLowerInvariant()
        $receipt = [ordered]@{
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
            msbuild_version = $contract.MsBuildVersion
            link_version = $contract.LinkVersion
            vc_tools_version = $contract.VcToolsVersion
        }
        $receipt | ConvertTo-Json | Set-Content -LiteralPath $receiptPath -Encoding UTF8

        Assert-LibsodiumBuildReceipt -ReceiptPath $receiptPath -LibraryPath $libraryPath | Out-Null

        [System.IO.File]::WriteAllBytes($libraryPath, [byte[]] (4, 3, 2, 1))
        Assert-Throws {
            Assert-LibsodiumBuildReceipt -ReceiptPath $receiptPath -LibraryPath $libraryPath
        } 'library SHA-256'
    }

    Invoke-Test 'receipt validation rejects a stale source contract' {
        $contract = Get-WindowsNativeBuildContract
        $libraryPath = Join-Path $testRoot 'stale-libsodium.lib'
        $receiptPath = Join-Path $testRoot 'stale-receipt.json'
        [System.IO.File]::WriteAllBytes($libraryPath, [byte[]] (5, 6, 7, 8))
        $libraryHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $libraryPath).Hash.ToLowerInvariant()
        $receipt = [ordered]@{
            schema_version = $contract.SchemaVersion
            recipe_revision = $contract.RecipeRevision
            libsodium_version = $contract.LibsodiumVersion
            source_asset = $contract.SourceAsset
            source_url = $contract.SourceUrl
            source_sha256 = ('0' * 64)
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
            msbuild_version = $contract.MsBuildVersion
            link_version = $contract.LinkVersion
            vc_tools_version = $contract.VcToolsVersion
        }
        $receipt | ConvertTo-Json | Set-Content -LiteralPath $receiptPath -Encoding UTF8

        Assert-Throws {
            Assert-LibsodiumBuildReceipt -ReceiptPath $receiptPath -LibraryPath $libraryPath
        } 'source_sha256'
    }

    Invoke-Test 'receipt validation rejects unknown JSON fields' {
        $libraryPath = Join-Path $testRoot 'unknown-field-libsodium.lib'
        $receiptPath = Join-Path $testRoot 'unknown-field-receipt.json'
        [System.IO.File]::WriteAllBytes($libraryPath, [byte[]] (10, 11, 12))
        $receipt = New-TestReceipt -LibraryPath $libraryPath -Extra @{ unexpected_field = 'contamination' }
        [System.IO.File]::WriteAllText($receiptPath, ($receipt | ConvertTo-Json))

        Assert-Throws {
            Assert-LibsodiumBuildReceipt -ReceiptPath $receiptPath -LibraryPath $libraryPath
        } 'unknown.*unexpected_field'
    }

    Invoke-Test 'receipt validation rejects missing JSON fields' {
        $libraryPath = Join-Path $testRoot 'missing-field-libsodium.lib'
        $receiptPath = Join-Path $testRoot 'missing-field-receipt.json'
        [System.IO.File]::WriteAllBytes($libraryPath, [byte[]] (19, 20, 21))
        $receipt = New-TestReceipt -LibraryPath $libraryPath -Omit @('visual_studio_installation_version')
        [System.IO.File]::WriteAllText($receiptPath, ($receipt | ConvertTo-Json))

        Assert-Throws {
            Assert-LibsodiumBuildReceipt -ReceiptPath $receiptPath -LibraryPath $libraryPath
        } 'missing.*visual_studio_installation_version'
    }

    Invoke-Test 'receipt validation rejects wrong JSON types and non-integral numbers' {
        $libraryPath = Join-Path $testRoot 'wrong-type-libsodium.lib'
        [System.IO.File]::WriteAllBytes($libraryPath, [byte[]] (13, 14, 15))
        $cases = @(
            @{ Field = 'schema_version'; Value = (Get-WindowsNativeBuildContract).SchemaVersion.ToString(); Pattern = 'schema_version.*JSON unsigned integer' },
            @{ Field = 'property_transform_count'; Value = 1.0; Pattern = 'property_transform_count.*JSON unsigned integer' },
            @{ Field = 'debug_property_transform_count'; Value = 1.5; Pattern = 'debug_property_transform_count.*JSON unsigned integer' },
            @{ Field = 'machine'; Value = 64; Pattern = 'machine.*JSON string' }
        )

        foreach ($case in $cases) {
            $receiptPath = Join-Path $testRoot "wrong-type-$($case.Field).json"
            $receipt = New-TestReceipt -LibraryPath $libraryPath -Overrides @{ $case.Field = $case.Value }
            $receiptJson = $receipt | ConvertTo-Json
            if ($case.Field -ceq 'property_transform_count') {
                $receiptJson = [regex]::Replace(
                    $receiptJson,
                    '"property_transform_count"\s*:\s*1(?=\s*[,}])',
                    '"property_transform_count": 1.0'
                )
            }
            [System.IO.File]::WriteAllText($receiptPath, $receiptJson)
            Assert-Throws {
                Assert-LibsodiumBuildReceipt -ReceiptPath $receiptPath -LibraryPath $libraryPath
            } $case.Pattern
        }
    }

    Invoke-Test 'receipt validation rejects stale recipes and every changed tool version' {
        $libraryPath = Join-Path $testRoot 'stale-toolchain-libsodium.lib'
        [System.IO.File]::WriteAllBytes($libraryPath, [byte[]] (16, 17, 18))
        $contract = Get-WindowsNativeBuildContract
        $cases = @(
            @{ Field = 'recipe_revision'; Value = $contract.RecipeRevision - 1 },
            @{ Field = 'visual_studio_installation_version'; Value = '17.15.10000.0' },
            @{ Field = 'msbuild_version'; Value = '17.14.99.99999' },
            @{ Field = 'vc_tools_version'; Value = '14.45.99999' },
            @{ Field = 'link_version'; Value = '14.45.99999.0' }
        )

        foreach ($case in $cases) {
            $receiptPath = Join-Path $testRoot "stale-$($case.Field).json"
            $receipt = New-TestReceipt -LibraryPath $libraryPath -Overrides @{ $case.Field = $case.Value }
            [System.IO.File]::WriteAllText($receiptPath, ($receipt | ConvertTo-Json))
            Assert-Throws {
                Assert-LibsodiumBuildReceipt -ReceiptPath $receiptPath -LibraryPath $libraryPath
            } $case.Field
        }
    }

    Invoke-Test 'the Rust build contract rejects unsupported Windows targets and suppresses no linker warnings' {
        $manifestRoot = Split-Path -Parent $scriptsRoot
        $buildScriptPath = Join-Path $manifestRoot 'build.rs'
        $cargoManifestPath = Join-Path $manifestRoot 'Cargo.toml'
        $buildScript = [System.IO.File]::ReadAllText($buildScriptPath)
        $cargoManifest = [System.IO.File]::ReadAllText($cargoManifestPath)

        foreach ($requiredText in @(
            'DESKTOP_LIBSODIUM_RECEIPT',
            'DESKTOP_MSVC_LINK',
            '17.14.37516.0',
            '17.14.51.32402',
            '14.44.35207',
            '14.44.35228.0',
            'visual_studio_installation_version',
            'VCToolsInstallDir',
            'library_sha256',
            'source_sha256',
            'runtime_library',
            'MultiThreadedDLL',
            '/dump',
            '/directives',
            '/headers',
            'sha2',
            'serde_json'
        )) {
            if (($buildScript + $cargoManifest) -notmatch [regex]::Escape($requiredText)) {
                throw "The Rust build contract does not contain '$requiredText'."
            }
        }

        if ($buildScript -notmatch '#\[serde\(deny_unknown_fields\)\]' -or
            $buildScript -notmatch [regex]::Escape('require_receipt_value("recipe_revision", receipt.recipe_revision, 3)')) {
            throw 'The Rust receipt schema is not closed and bound to recipe revision 3.'
        }

        if ($buildScript -match '/IGNORE:\d+' -or $buildScript -match 'linker_messages') {
            throw 'The Rust build suppresses a linker diagnostic.'
        }
        foreach ($requiredTargetText in @('CARGO_CFG_TARGET_OS', 'CARGO_CFG_TARGET_ENV', 'CARGO_CFG_TARGET_ARCH', 'unsupported Windows target')) {
            if ($buildScript -notmatch [regex]::Escape($requiredTargetText)) {
                throw "The Rust build does not fail closed on unsupported Windows targets: missing '$requiredTargetText'."
            }
        }
        if ($buildScript -notmatch [regex]::Escape('env::var("TARGET")')) {
            throw 'The Rust build does not require the exact Cargo target triple.'
        }
    }

    Invoke-Test 'the release property transform embeds debug information instead of requiring a PDB' {
        $propertyPath = Join-Path $testRoot 'Release.props'
        $source = '<Project><DebugInformationFormat>ProgramDatabase</DebugInformationFormat></Project>'
        [System.IO.File]::WriteAllText($propertyPath, $source)

        $count = Convert-LibsodiumReleaseDebugInformationToEmbedded -Path $propertyPath
        $transformed = [System.IO.File]::ReadAllText($propertyPath)

        Assert-Equal 1 $count 'Unexpected debug-information replacement count.'
        Assert-Equal 0 ([regex]::Matches($transformed, '<DebugInformationFormat>ProgramDatabase</DebugInformationFormat>').Count) 'The external PDB setting remains.'
        Assert-Equal 1 ([regex]::Matches($transformed, '<DebugInformationFormat>OldStyle</DebugInformationFormat>').Count) 'The embedded debug-information setting is missing.'
    }

    Invoke-Test 'the source build uses a bounded project-local staging prefix for MSVC paths' {
        $moduleSource = [System.IO.File]::ReadAllText($modulePath)
        if ($moduleSource -notmatch [regex]::Escape("`$workRoot = Join-Path `$NativeRoot 'w'")) {
            throw 'The source staging prefix is not the bounded .native/w path.'
        }
        if ($moduleSource -match "\.work\." -or $moduleSource -match "Join-Path `$workRoot 'extract'") {
            throw 'The source staging layout adds path components that can exceed MSVC path limits.'
        }
    }

    Invoke-Test 'one Visual Studio 2022 installation supplies VsDevCmd MSBuild link and v143' {
        $installation = Join-Path $testRoot 'vs2022'
        $developerCommand = Join-Path $installation 'Common7\Tools\VsDevCmd.bat'
        $msbuild = Join-Path $installation 'MSBuild\Current\Bin\MSBuild.exe'
        $toolset = Join-Path $installation 'MSBuild\Microsoft\VC\v170\Platforms\x64\PlatformToolsets\v143\Toolset.props'
        $olderLink = Join-Path $installation 'VC\Tools\MSVC\14.40.10000\bin\Hostx64\x64\link.exe'
        $newerLink = Join-Path $installation 'VC\Tools\MSVC\14.44.35207\bin\Hostx64\x64\link.exe'
        foreach ($file in @($developerCommand, $msbuild, $toolset, $olderLink, $newerLink)) {
            New-Item -ItemType Directory -Path (Split-Path -Parent $file) -Force | Out-Null
            [System.IO.File]::WriteAllText($file, '')
        }

        $toolchain = Get-VisualStudio2022Toolchain -InstallationPath $installation

        Assert-Equal $developerCommand $toolchain.DeveloperCommand 'VsDevCmd came from another installation.'
        Assert-Equal $msbuild $toolchain.MsBuildPath 'MSBuild came from another installation.'
        Assert-Equal $newerLink $toolchain.LinkPath 'The latest x64 linker in the selected installation was not chosen.'
        Assert-Equal $toolset $toolchain.V143ToolsetPath 'The v143 marker came from another installation.'
    }

    Invoke-Test 'Visual Studio toolchain selection rejects a missing v143 toolset' {
        $installation = Join-Path $testRoot 'vs-without-v143'
        foreach ($file in @(
            (Join-Path $installation 'Common7\Tools\VsDevCmd.bat'),
            (Join-Path $installation 'MSBuild\Current\Bin\MSBuild.exe'),
            (Join-Path $installation 'VC\Tools\MSVC\14.44.35207\bin\Hostx64\x64\link.exe')
        )) {
            New-Item -ItemType Directory -Path (Split-Path -Parent $file) -Force | Out-Null
            [System.IO.File]::WriteAllText($file, '')
        }

        Assert-Throws {
            Get-VisualStudio2022Toolchain -InstallationPath $installation
        } 'v143'
    }

    Invoke-Test 'Visual Studio discovery accepts only the approved installation version' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $installation = Join-Path $testRoot 'approved-vs'
        New-Item -ItemType Directory -Path $installation -Force | Out-Null
        $escapedInstallation = $installation.Replace('\', '\\')

        $approvedVsWhere = Join-Path $testRoot 'approved-vswhere.cmd'
        [System.IO.File]::WriteAllText(
            $approvedVsWhere,
            "@echo off`r`necho [{`"installationPath`":`"$escapedInstallation`",`"installationVersion`":`"17.14.37516.0`"}]`r`n"
        )
        $selected = & $nativeBuildModule {
            param($vswhere)
            Get-VisualStudioInstallationPath -VsWherePath $vswhere
        } $approvedVsWhere
        Assert-Equal $installation $selected 'The approved Visual Studio installation was not selected.'

        $newerVsWhere = Join-Path $testRoot 'newer-vswhere.cmd'
        [System.IO.File]::WriteAllText(
            $newerVsWhere,
            "@echo off`r`necho [{`"installationPath`":`"$escapedInstallation`",`"installationVersion`":`"17.15.10000.0`"}]`r`n"
        )
        Assert-Throws {
            & $nativeBuildModule {
                param($vswhere)
                Get-VisualStudioInstallationPath -VsWherePath $vswhere
            } $newerVsWhere
        } '17\.14\.37516\.0'
    }

    Invoke-Test 'the approved toolchain version contract rejects every changed component' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $approved = [ordered]@{
            VisualStudioInstallationVersion = '17.14.37516.0'
            MsBuildVersion = '17.14.51.32402'
            VcToolsVersion = '14.44.35207'
            LinkVersion = '14.44.35228.0'
        }

        & $nativeBuildModule {
            param($versions)
            Assert-ApprovedToolchainVersions @versions
        } $approved

        foreach ($field in $approved.Keys) {
            $changed = @{}
            foreach ($entry in $approved.GetEnumerator()) {
                $changed[$entry.Key] = $entry.Value
            }
            $changed[$field] = '99.99.99999.0'
            Assert-Throws {
                & $nativeBuildModule {
                    param($versions)
                    Assert-ApprovedToolchainVersions @versions
                } $changed
            } $field
        }
    }

    Invoke-Test 'the linker help probe accepts only parseable approved output with exit 1100' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $fakeLinker = Join-Path $testRoot 'fake-linker-version.cmd'
        [System.IO.File]::WriteAllText(
            $fakeLinker,
            "@echo off`r`nif defined FAKE_LINK_VERSION_OUTPUT echo %FAKE_LINK_VERSION_OUTPUT%`r`nexit /b %FAKE_LINK_EXIT_CODE%`r`n"
        )
        $variables = @('FAKE_LINK_VERSION_OUTPUT', 'FAKE_LINK_EXIT_CODE')
        $previousValues = @{}
        try {
            foreach ($variable in $variables) {
                $previousValues[$variable] = [System.Environment]::GetEnvironmentVariable($variable, 'Process')
            }

            $env:FAKE_LINK_VERSION_OUTPUT = 'Microsoft (R) Incremental Linker Version 14.44.35228.0'
            $env:FAKE_LINK_EXIT_CODE = '1100'
            $version = & $nativeBuildModule {
                param($linker)
                Get-ApprovedMsvcLinkerVersion -LinkPath $linker
            } $fakeLinker
            Assert-Equal '14.44.35228.0' $version 'The approved linker help output was not accepted.'

            Assert-Throws {
                & $nativeBuildModule {
                    param($linker)
                    Get-ExecutableVersion -ExecutablePath $linker -Arguments @('/?') -DisplayName 'generic tool'
                } $fakeLinker
            } 'exit 1100'

            $env:FAKE_LINK_EXIT_CODE = '42'
            Assert-Throws {
                & $nativeBuildModule {
                    param($linker)
                    Get-ApprovedMsvcLinkerVersion -LinkPath $linker
                } $fakeLinker
            } 'exit 42'

            $env:FAKE_LINK_EXIT_CODE = '1100'
            $env:FAKE_LINK_VERSION_OUTPUT = $null
            Assert-Throws {
                & $nativeBuildModule {
                    param($linker)
                    Get-ApprovedMsvcLinkerVersion -LinkPath $linker
                } $fakeLinker
            } 'parseable version'

            $env:FAKE_LINK_VERSION_OUTPUT = 'Microsoft linker version unavailable'
            Assert-Throws {
                & $nativeBuildModule {
                    param($linker)
                    Get-ApprovedMsvcLinkerVersion -LinkPath $linker
                } $fakeLinker
            } 'parseable version'

            $env:FAKE_LINK_VERSION_OUTPUT = 'Microsoft (R) Incremental Linker Version 14.45.99999.0'
            Assert-Throws {
                & $nativeBuildModule {
                    param($linker)
                    Get-ApprovedMsvcLinkerVersion -LinkPath $linker
                } $fakeLinker
            } '14\.44\.35228\.0'
        }
        finally {
            foreach ($variable in $variables) {
                [System.Environment]::SetEnvironmentVariable($variable, $previousValues[$variable], 'Process')
            }
        }
    }

    Invoke-Test 'toolchain selection refuses an unapproved VC tools directory' {
        $installation = Join-Path $testRoot 'vs-with-newer-vc-tools'
        foreach ($file in @(
            (Join-Path $installation 'Common7\Tools\VsDevCmd.bat'),
            (Join-Path $installation 'MSBuild\Current\Bin\MSBuild.exe'),
            (Join-Path $installation 'MSBuild\Microsoft\VC\v170\Platforms\x64\PlatformToolsets\v143\Toolset.props'),
            (Join-Path $installation 'VC\Tools\MSVC\14.45.99999\bin\Hostx64\x64\link.exe')
        )) {
            New-Item -ItemType Directory -Path (Split-Path -Parent $file) -Force | Out-Null
            [System.IO.File]::WriteAllText($file, '')
        }

        Assert-Throws {
            Get-VisualStudio2022Toolchain -InstallationPath $installation
        } '14\.44\.35207'
    }

    Invoke-Test 'the controlled MSBuild boundary clears inherited inputs and isolates stdout' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $fakeMsBuild = Join-Path $testRoot 'fake-msbuild.cmd'
        $environmentCapture = Join-Path $testRoot 'msbuild-environment.txt'
        $argumentCapture = Join-Path $testRoot 'msbuild-arguments.txt'
        [System.IO.File]::WriteAllText(
            $fakeMsBuild,
            "@echo off`r`nset > `"$environmentCapture`"`r`necho %* > `"$argumentCapture`"`r`necho fixture-msbuild-stdout`r`nexit /b 0`r`n"
        )

        $contaminatingVariables = @(
            'CL',
            '_CL_',
            'LINK',
            '_LINK_',
            'ForceImportBeforeCppTargets',
            'ForceImportAfterCppTargets'
        )
        $previousValues = @{}
        try {
            foreach ($variable in $contaminatingVariables) {
                $previousValues[$variable] = [System.Environment]::GetEnvironmentVariable($variable, 'Process')
                [System.Environment]::SetEnvironmentVariable($variable, "contaminating-$variable", 'Process')
            }

            $result = @(& $nativeBuildModule {
                    param($msbuild, $solution, $log)
                    Invoke-ControlledMsBuild -MsBuildPath $msbuild -SolutionPath $solution -LogPath $log
                } $fakeMsBuild (Join-Path $testRoot 'fixture.sln') (Join-Path $testRoot 'fixture.log'))

            Assert-Equal 1 $result.Count 'MSBuild stdout contaminated the numeric return object.'
            Assert-Equal 0 $result[0] 'The fake MSBuild command did not succeed.'
            $capturedEnvironment = [System.IO.File]::ReadAllText($environmentCapture)
            foreach ($variable in $contaminatingVariables) {
                if ($capturedEnvironment -match "(?im)^$([regex]::Escape($variable))=") {
                    throw "The controlled MSBuild process inherited $variable."
                }
            }

            $capturedArguments = [System.IO.File]::ReadAllText($argumentCapture)
            foreach ($requiredArgument in @('-noAutoResponse', 'ImportDirectoryBuildProps=false', 'ImportDirectoryBuildTargets=false')) {
                if ($capturedArguments -notmatch [regex]::Escape($requiredArgument)) {
                    throw "The controlled MSBuild process did not receive $requiredArgument."
                }
            }
        }
        finally {
            foreach ($variable in $contaminatingVariables) {
                [System.Environment]::SetEnvironmentVariable($variable, $previousValues[$variable], 'Process')
            }
        }
    }

    Invoke-Test 'Cargo native output validation rejects bundled fallback contamination' {
        $targetRoot = Join-Path $testRoot 'target'
        $profileRoot = Join-Path $targetRoot 'debug'
        $buildRoot = Join-Path $profileRoot 'build\libsodium-sys-stable-fixture'
        $outputPath = Join-Path $buildRoot 'output'
        $libraryDirectory = Join-Path $testRoot 'artifact\lib'
        New-Item -ItemType Directory -Path $buildRoot -Force | Out-Null
        New-Item -ItemType Directory -Path $libraryDirectory -Force | Out-Null
        [System.IO.File]::WriteAllText($outputPath, "cargo:rustc-link-search=native=$libraryDirectory`r`n")
        Assert-CargoUsedProvisionedLibsodium -BuildRoot $profileRoot -LibraryDirectory $libraryDirectory

        $fallbackDirectory = Join-Path $testRoot 'bundled\out\installed\lib'
        [System.IO.File]::WriteAllText($outputPath, "cargo:rustc-link-search=native=$fallbackDirectory")
        Assert-Throws {
            Assert-CargoUsedProvisionedLibsodium -BuildRoot $profileRoot -LibraryDirectory $libraryDirectory
        } 'provisioned libsodium'
    }

    Invoke-Test 'the selective clean follows the requested Cargo profile' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $releaseArguments = @(& $nativeBuildModule {
                Get-CargoCleanProfileArguments -CargoArguments @('test', '--release', '--test', 'storage')
            })
        Assert-Equal 1 $releaseArguments.Count 'The release clean profile argument count is wrong.'
        Assert-Equal '--release' $releaseArguments[0] 'The release profile was not forwarded to cargo clean.'

        $customArguments = @(& $nativeBuildModule {
                Get-CargoCleanProfileArguments -CargoArguments @('build', '--profile', 'production')
            })
        Assert-Equal 2 $customArguments.Count 'The custom clean profile argument count is wrong.'
        Assert-Equal '--profile' $customArguments[0] 'The custom profile flag was not forwarded.'
        Assert-Equal 'production' $customArguments[1] 'The custom profile name was not forwarded.'
    }

    Invoke-Test 'Cargo target and target-directory parsing stop at the application boundary' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $parsed = & $nativeBuildModule {
            [pscustomobject]@{
                Target = Get-CargoTargetArgument -CargoArguments @('test', '--target', 'x86_64-pc-windows-msvc', '--', '--target', 'malicious')
                TargetDirectory = Get-CargoTargetDirectoryArgument -CargoArguments @('test', '--target-dir', 'trusted', '--', '--target-dir', 'malicious')
                ProfileDirectory = Get-CargoProfileDirectory -CargoArguments @('test', '--release', '--', '--profile', 'malicious')
            }
        }

        Assert-Equal 'x86_64-pc-windows-msvc' $parsed.Target 'The explicit Cargo target was parsed incorrectly.'
        Assert-Equal 'trusted' $parsed.TargetDirectory 'Application arguments changed the Cargo target directory.'
        Assert-Equal 'release' $parsed.ProfileDirectory 'Application arguments changed the Cargo profile directory.'
    }

    Invoke-Test 'the default controlled Cargo target root uses the intentional short name' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $manifestRoot = Join-Path $testRoot 'default-target-root'
        $targetRoot = & $nativeBuildModule {
            param($root)
            Get-CargoTargetRoot -ManifestRoot $root -CargoArguments @('build', '--release')
        } $manifestRoot

        Assert-Equal (Join-Path $manifestRoot '.native\t') $targetRoot 'The default Cargo target root is not the intentional short path.'
    }

    Invoke-Test 'the OpenSSL object path budget accepts a 259-character probe' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $targetRoot = New-TestCargoTargetRootForProbeLength -ProbeLength 259
        $probePath = Join-Path $targetRoot "x86_64-pc-windows-msvc\release\$script:openSslObjectProbeSuffix"
        Assert-Equal 259 $probePath.Length 'The accepted boundary fixture does not model a 259-character path.'

        & $nativeBuildModule {
            param($root)
            Assert-CargoTargetPathBudget -TargetRoot $root -CargoArguments @('build', '--release')
        } $targetRoot
    }

    Invoke-Test 'the OpenSSL object path budget rejects a 260-character probe' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $targetRoot = New-TestCargoTargetRootForProbeLength -ProbeLength 260
        $probePath = Join-Path $targetRoot "x86_64-pc-windows-msvc\release\$script:openSslObjectProbeSuffix"
        Assert-Equal 260 $probePath.Length 'The rejected boundary fixture does not model a 260-character path.'

        Assert-Throws {
            & $nativeBuildModule {
                param($root)
                Assert-CargoTargetPathBudget -TargetRoot $root -CargoArguments @('build', '--release')
            } $targetRoot
        } '260.*shorter absolute --target-dir'
    }

    Invoke-Test 'the controlled Cargo environment overrides configured native-build inputs' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $manifestRoot = Join-Path $testRoot 'cargo-environment'
        $selectedLinker = Join-Path $testRoot 'approved-link.exe'
        New-Item -ItemType Directory -Path $manifestRoot -Force | Out-Null
        [System.IO.File]::WriteAllText($selectedLinker, '')
        $variables = @(
            'CARGO_BUILD_TARGET',
            'CARGO_TARGET_DIR',
            'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER',
            'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS',
            'CARGO_BUILD_RUSTFLAGS',
            'CARGO_ENCODED_RUSTFLAGS',
            'RUSTFLAGS',
            'CFLAGS',
            'CXXFLAGS'
        )
        $previousValues = @{}
        try {
            foreach ($variable in $variables) {
                $previousValues[$variable] = [System.Environment]::GetEnvironmentVariable($variable, 'Process')
                [System.Environment]::SetEnvironmentVariable($variable, 'ambient-contamination', 'Process')
            }

            $invocation = & $nativeBuildModule {
                param($root, $linker)
                New-ControlledCargoInvocation `
                    -ManifestRoot $root `
                    -LinkPath $linker `
                    -CargoArguments @('test', '--', '--target', 'application-value')
            } $manifestRoot $selectedLinker

            $contract = Get-WindowsNativeBuildContract
            $expectedTargetRoot = Join-Path $manifestRoot '.native\t'
            Assert-Equal $contract.CargoTarget $invocation.Target 'The Cargo target was not pinned.'
            Assert-Equal $expectedTargetRoot $invocation.TargetRoot 'The default Cargo target directory was not controlled.'
            Assert-Equal (Join-Path $expectedTargetRoot "$($contract.CargoTarget)\debug") $invocation.BuildRoot 'The attestation root does not match the controlled target and profile.'
            Assert-Equal $contract.CargoTarget $env:CARGO_BUILD_TARGET 'Ambient Cargo build.target won.'
            Assert-Equal $expectedTargetRoot $env:CARGO_TARGET_DIR 'Ambient Cargo target-dir won.'
            Assert-Equal $selectedLinker $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER 'The selected linker was not pinned through the target environment.'
            Assert-Equal '-Ctarget-feature=-crt-static' $env:CARGO_ENCODED_RUSTFLAGS 'The controlled dynamic-CRT rustflag is wrong.'
            Assert-Equal '/Z7' $env:CFLAGS 'Ambient OpenSSL CFLAGS were not replaced.'
            Assert-Equal '/Z7' $env:CXXFLAGS 'Ambient OpenSSL CXXFLAGS were not replaced.'
            foreach ($clearedVariable in @('RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS', 'CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS')) {
                if ([System.Environment]::GetEnvironmentVariable($clearedVariable, 'Process')) {
                    throw "$clearedVariable was not cleared."
                }
            }

            $boundary = [array]::IndexOf($invocation.CargoArguments, '--')
            $targetFlag = [array]::IndexOf($invocation.CargoArguments, '--target')
            $targetDirectoryFlag = [array]::IndexOf($invocation.CargoArguments, '--target-dir')
            if ($targetFlag -lt 0 -or $targetFlag -gt $boundary) {
                throw 'The explicit Cargo target was not inserted before application arguments.'
            }
            if ($targetDirectoryFlag -lt 0 -or $targetDirectoryFlag -gt $boundary) {
                throw 'The controlled Cargo target directory was not inserted before application arguments.'
            }
            Assert-Equal $contract.CargoTarget $invocation.CargoArguments[$targetFlag + 1] 'The explicit Cargo target argument is wrong.'
            Assert-Equal $expectedTargetRoot $invocation.CargoArguments[$targetDirectoryFlag + 1] 'The explicit Cargo target-dir argument is wrong.'
        }
        finally {
            foreach ($variable in $variables) {
                [System.Environment]::SetEnvironmentVariable($variable, $previousValues[$variable], 'Process')
            }
        }
    }

    Invoke-Test 'the Cargo release alias selects the exact target profile build root' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $manifestRoot = Join-Path $testRoot 'cargo-release-alias'
        $customTargetRoot = Join-Path $testRoot 'custom-cargo-target'
        $selectedLinker = Join-Path $testRoot 'release-link.exe'
        New-Item -ItemType Directory -Path $manifestRoot -Force | Out-Null
        [System.IO.File]::WriteAllText($selectedLinker, '')

        $invocation = & $nativeBuildModule {
            param($root, $linker, $targetRoot)
            New-ControlledCargoInvocation `
                -ManifestRoot $root `
                -LinkPath $linker `
                -CargoArguments @('build', '-r', '--target-dir', $targetRoot)
        } $manifestRoot $selectedLinker $customTargetRoot

        $contract = Get-WindowsNativeBuildContract
        Assert-Equal 'release' $invocation.ProfileDirectory 'Cargo -r was not recognized as the release profile.'
        Assert-Equal $customTargetRoot $invocation.TargetRoot 'The caller target-dir was not preserved.'
        Assert-Equal (Join-Path $customTargetRoot "$($contract.CargoTarget)\release") $invocation.BuildRoot 'The release attestation root is wrong.'
    }

    Invoke-Test 'the Cargo boundary rejects command-level native override escape hatches' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $manifestRoot = Join-Path $testRoot 'cargo-rejections'
        $selectedLinker = Join-Path $testRoot 'rejection-link.exe'
        New-Item -ItemType Directory -Path $manifestRoot -Force | Out-Null
        [System.IO.File]::WriteAllText($selectedLinker, '')
        $cases = @(
            @{ Arguments = @('rustc', '--', '-Ctarget-feature=-crt-static'); Pattern = 'cargo rustc' },
            @{ Arguments = @('--color', 'always', 'rustc', '--', '-Ctarget-feature=-crt-static'); Pattern = 'cargo rustc' },
            @{ Arguments = @('build', '--config', 'build.rustflags=["-Clinker=evil"]'); Pattern = '--config' },
            @{ Arguments = @('build', '--config=build.rustflags=["-Clinker=evil"]'); Pattern = '--config' },
            @{ Arguments = @('build', '--codegen', 'linker=evil-link.exe'); Pattern = 'linker' },
            @{ Arguments = @('build', '-C', 'target-feature=+crt-static'); Pattern = 'crt-static' },
            @{ Arguments = @('build', '--target', 'aarch64-pc-windows-msvc'); Pattern = 'only x86_64-pc-windows-msvc' },
            @{ Arguments = @('build', '--target', 'x86_64-pc-windows-msvc', '--target', 'aarch64-pc-windows-msvc'); Pattern = '--target may be supplied only once' },
            @{ Arguments = @('build', '--target-dir', 'trusted', '--target-dir', 'malicious'); Pattern = '--target-dir may be supplied only once' },
            @{ Arguments = @('build', '--target', ''); Pattern = 'empty string|--target requires' },
            @{ Arguments = @('build', '--target-dir', ''); Pattern = 'empty string|--target-dir is missing' }
        )

        foreach ($case in $cases) {
            Assert-Throws {
                & $nativeBuildModule {
                    param($root, $linker, $arguments)
                    New-ControlledCargoInvocation -ManifestRoot $root -LinkPath $linker -CargoArguments $arguments
                } $manifestRoot $selectedLinker ([string[]] $case.Arguments)
            } $case.Pattern
        }
    }

    Invoke-Test 'cargo clean uses the requested target profile and target directory contract' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $manifestRoot = Join-Path $testRoot 'cargo-clean-contract'
        $customTargetRoot = Join-Path $testRoot 'cargo-clean-target'
        $selectedLinker = Join-Path $testRoot 'clean-link.exe'
        New-Item -ItemType Directory -Path $manifestRoot -Force | Out-Null
        [System.IO.File]::WriteAllText($selectedLinker, '')

        $result = & $nativeBuildModule {
            param($root, $linker, $targetRoot)
            $invocation = New-ControlledCargoInvocation `
                -ManifestRoot $root `
                -LinkPath $linker `
                -CargoArguments @('test', '-r', '--target-dir', $targetRoot, '--test', 'storage')
            $cleanArguments = [string[]] @(Get-LibsodiumCargoCleanArguments `
                    -ManifestRoot $root `
                    -CargoArguments $invocation.CargoArguments)
            [pscustomobject]@{
                Invocation = $invocation
                CleanArguments = $cleanArguments
                CleanTarget = Get-CargoTargetArgument -CargoArguments $cleanArguments
                CleanTargetRoot = Get-CargoTargetDirectoryArgument -CargoArguments $cleanArguments
                CleanProfile = Get-CargoProfileDirectory -CargoArguments $cleanArguments
            }
        } $manifestRoot $selectedLinker $customTargetRoot

        Assert-Equal $result.Invocation.Target $result.CleanTarget 'cargo clean used a different target.'
        Assert-Equal $result.Invocation.TargetRoot $result.CleanTargetRoot 'cargo clean used a different target directory.'
        Assert-Equal $result.Invocation.ProfileDirectory $result.CleanProfile 'cargo clean used a different profile.'
        Assert-Equal 'clean' $result.CleanArguments[0] 'The package-scoped command is not cargo clean.'
        if ($result.CleanArguments -notcontains 'libsodium-sys-stable') {
            throw 'The clean command is not scoped to libsodium-sys-stable.'
        }
    }

    Invoke-Test 'the controlled Cargo boundary cleans builds and attests the exact root without leaking stdout' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $manifestRoot = Join-Path $testRoot 'cargo-boundary'
        $targetRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("wnb-cargo-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
        $libraryDirectory = Join-Path $testRoot 'cargo-boundary-libsodium\lib'
        $selectedLinker = Join-Path $testRoot 'boundary-link.exe'
        $callsPath = Join-Path $testRoot 'fake-cargo-calls.txt'
        $fakeCargo = Join-Path $testRoot 'fake-cargo.cmd'
        $contract = Get-WindowsNativeBuildContract
        $buildRoot = Join-Path $targetRoot "$($contract.CargoTarget)\release"
        New-Item -ItemType Directory -Path $manifestRoot, $libraryDirectory -Force | Out-Null
        [System.IO.File]::WriteAllText($selectedLinker, '')
        [System.IO.File]::WriteAllText(
            $fakeCargo,
            @'
@echo off
echo %*>>"%FAKE_CARGO_CALLS%"
if "%1"=="clean" exit /b 0
mkdir "%FAKE_CARGO_BUILD_ROOT%\build\libsodium-sys-stable-fixture" 2>nul
echo cargo:rustc-link-search=native=%FAKE_CARGO_LIBRARY_DIRECTORY%>"%FAKE_CARGO_BUILD_ROOT%\build\libsodium-sys-stable-fixture\output"
echo fixture-cargo-stdout
exit /b 0
'@
        )

        $decoyRoot = Join-Path $manifestRoot 'target\release\build\libsodium-sys-stable-decoy'
        New-Item -ItemType Directory -Path $decoyRoot -Force | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $decoyRoot 'output'), 'cargo:rustc-link-search=native=C:\decoy')
        $fakeVariables = @('FAKE_CARGO_CALLS', 'FAKE_CARGO_BUILD_ROOT', 'FAKE_CARGO_LIBRARY_DIRECTORY')
        $previousValues = @{}
        try {
            foreach ($variable in $fakeVariables) {
                $previousValues[$variable] = [System.Environment]::GetEnvironmentVariable($variable, 'Process')
            }
            $env:FAKE_CARGO_CALLS = $callsPath
            $env:FAKE_CARGO_BUILD_ROOT = $buildRoot
            $env:FAKE_CARGO_LIBRARY_DIRECTORY = $libraryDirectory

            $result = @(& $nativeBuildModule {
                    param($cargo, $root, $linker, $targetDirectory, $library)
                    Invoke-ControlledCargoBuild `
                        -CargoPath $cargo `
                        -ManifestRoot $root `
                        -LinkPath $linker `
                        -CargoArguments @('build', '-r', '--target-dir', $targetDirectory) `
                        -LibraryDirectory $library
                } $fakeCargo $manifestRoot $selectedLinker $targetRoot $libraryDirectory)

            Assert-Equal 1 $result.Count 'Cargo stdout contaminated the numeric return object.'
            Assert-Equal 0 $result[0] 'The fake Cargo build did not succeed.'
            $calls = @(Get-Content -LiteralPath $callsPath)
            Assert-Equal 2 $calls.Count 'The Cargo boundary did not run one clean and one requested command.'
            foreach ($call in $calls) {
                foreach ($requiredArgument in @($contract.CargoTarget, $targetRoot)) {
                    if ($call -notmatch [regex]::Escape($requiredArgument)) {
                        throw "Cargo command '$call' did not use '$requiredArgument'."
                    }
                }
            }
            if ($calls[0] -notmatch '(?:^|\s)clean(?:\s|$)' -or $calls[0] -notmatch '(?:^|\s)--release(?:\s|$)') {
                throw 'The selective clean did not use the release profile.'
            }
            if ($calls[1] -notmatch '(?:^|\s)-r(?:\s|$)') {
                throw 'The requested Cargo command lost its release alias.'
            }
        }
        finally {
            foreach ($variable in $fakeVariables) {
                [System.Environment]::SetEnvironmentVariable($variable, $previousValues[$variable], 'Process')
            }
            Remove-Item -LiteralPath $targetRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
    }

    Invoke-Test 'an over-budget target directory is rejected before invoking Cargo' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $manifestRoot = Join-Path $testRoot 'cargo-path-budget-rejection'
        $targetRoot = New-TestCargoTargetRootForProbeLength -ProbeLength 260
        $selectedLinker = Join-Path $testRoot 'path-budget-link.exe'
        $libraryDirectory = Join-Path $testRoot 'path-budget-libsodium\lib'
        $callsPath = Join-Path $testRoot 'path-budget-cargo-calls.txt'
        $fakeCargo = Join-Path $testRoot 'path-budget-cargo.cmd'
        New-Item -ItemType Directory -Path $manifestRoot, $libraryDirectory -Force | Out-Null
        [System.IO.File]::WriteAllText($selectedLinker, '')
        [System.IO.File]::WriteAllText(
            $fakeCargo,
            "@echo off`r`necho invoked>>`"$callsPath`"`r`nexit /b 0`r`n"
        )

        Assert-Throws {
            & $nativeBuildModule {
                param($cargo, $root, $linker, $targetDirectory, $library)
                Invoke-ControlledCargoBuild `
                    -CargoPath $cargo `
                    -ManifestRoot $root `
                    -LinkPath $linker `
                    -CargoArguments @('build', '--release', '--target-dir', $targetDirectory) `
                    -LibraryDirectory $library
            } $fakeCargo $manifestRoot $selectedLinker $targetRoot $libraryDirectory
        } '260.*shorter absolute --target-dir'

        Assert-Equal $false (Test-Path -LiteralPath $callsPath) 'Cargo was invoked before the target path budget rejection.'
    }

    Invoke-Test 'Cargo build-root scoping always includes the approved explicit target' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $targetRoot = Join-Path $testRoot 'cargo-target'
        $roots = & $nativeBuildModule {
            param($targetRoot)
            [pscustomobject]@{
                Host = Get-CargoBuildRoot -TargetRoot $targetRoot -CargoArguments @('test', '--release')
                Explicit = Get-CargoBuildRoot -TargetRoot $targetRoot -CargoArguments @('test', '--release', '--target', 'x86_64-pc-windows-msvc')
            }
        } $targetRoot

        Assert-Equal (Join-Path $targetRoot 'x86_64-pc-windows-msvc\release') $roots.Host 'The implicit request was not pinned to the approved target.'
        Assert-Equal (Join-Path $targetRoot 'x86_64-pc-windows-msvc\release') $roots.Explicit 'The explicit target build root is not scoped to its target triple.'
    }

    Invoke-Test 'native command forwarding tolerates successful stderr and returns only the exit code' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $exitCode = & $nativeBuildModule {
            param($executable, $arguments)
            Invoke-NativeCommandWithOutput -ExecutablePath $executable -Arguments $arguments
        } $env:ComSpec ([string[]] @('/d', '/s', '/c', 'echo fixture-stderr 1>&2'))

        Assert-Equal 0 $exitCode 'Successful native stderr contaminated the numeric return value.'
    }

    Invoke-Test 'provisioned cache publication validates in module scope and removes the backup' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $nativeRoot = Join-Path $testRoot 'validated-publication-cache'
        $artifactRoot = Join-Path $nativeRoot 'artifact'
        $stagedRoot = Join-Path $nativeRoot 'staged'
        $stageLibraryDirectory = Join-Path $stagedRoot 'lib'
        $stageLibrary = Join-Path $stageLibraryDirectory 'libsodium.lib'
        $fakeLink = Join-Path $testRoot 'publication-link.cmd'
        $fakeMsBuild = Join-Path $testRoot 'publication-msbuild.cmd'
        $contract = Get-WindowsNativeBuildContract

        New-Item -ItemType Directory -Path $artifactRoot, $stageLibraryDirectory -Force | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $artifactRoot 'marker.txt'), 'previous')
        [System.IO.File]::WriteAllBytes($stageLibrary, [byte[]] (31, 32, 33, 34))
        $receipt = New-TestReceipt -LibraryPath $stageLibrary
        [System.IO.File]::WriteAllText((Join-Path $stagedRoot 'receipt.json'), ($receipt | ConvertTo-Json))
        [System.IO.File]::WriteAllText(
            $fakeLink,
            "@echo off`r`n" +
            "if /I `%~2==/directives (`r`n" +
            "  echo Microsoft ^(R^) COFF/PE Dumper Version $($contract.LinkVersion)`r`n" +
            "  echo /DEFAULTLIB:^`"MSVCRT^`"`r`n" +
            "  exit /b 0`r`n" +
            ")`r`n" +
            "if /I `%~2==/headers (`r`n" +
            "  echo Microsoft ^(R^) COFF/PE Dumper Version $($contract.LinkVersion)`r`n" +
            "  echo 8664 machine ^(x64^)`r`n" +
            "  exit /b 0`r`n" +
            ")`r`n" +
            "exit /b 1`r`n"
        )
        [System.IO.File]::WriteAllText(
            $fakeMsBuild,
            "@echo off`r`necho $($contract.MsBuildVersion)`r`nexit /b 0`r`n"
        )

        $published = & $nativeBuildModule {
            param($native, $staged, $artifact, $link, $msbuild, $vcToolsVersion)
            Publish-ProvisionedLibsodiumArtifact `
                -NativeRoot $native `
                -StagedArtifactRoot $staged `
                -ArtifactRoot $artifact `
                -LinkPath $link `
                -MsBuildPath $msbuild `
                -VcToolsVersion $vcToolsVersion
        } $nativeRoot $stagedRoot $artifactRoot $fakeLink $fakeMsBuild $contract.VcToolsVersion

        Assert-Equal $artifactRoot $published.ArtifactRoot 'Publication did not return the validated artifact root.'
        Assert-Equal (Join-Path $artifactRoot 'lib\libsodium.lib') $published.LibraryPath 'Publication returned the wrong validated library path.'
        Assert-Equal $true (Test-Path -LiteralPath $published.ReceiptPath -PathType Leaf) 'Publication did not return a valid receipt path.'
        Assert-Equal $false (Test-Path -LiteralPath (Join-Path $artifactRoot 'marker.txt')) 'The previous cache remained installed.'
        Assert-Equal 0 @(Get-ChildItem -LiteralPath $nativeRoot -Directory | Where-Object { $_.Name -like 'backup-*' }).Count 'A successful publication left its backup behind.'
    }

    Invoke-Test 'backup cleanup failure leaves the validated replacement committed' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $nativeRoot = Join-Path $testRoot 'cleanup-failure-cache'
        $artifactRoot = Join-Path $nativeRoot 'artifact'
        $stagedRoot = Join-Path $nativeRoot 'staged'
        New-Item -ItemType Directory -Path $artifactRoot, $stagedRoot -Force | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $artifactRoot 'marker.txt'), 'previous')
        [System.IO.File]::WriteAllText((Join-Path $artifactRoot 'second.txt'), 'backup-remnant')
        [System.IO.File]::WriteAllText((Join-Path $stagedRoot 'marker.txt'), 'validated-replacement')

        $operations = [System.Collections.Generic.List[string]]::new()
        $moveDirectory = {
            param($source, $destination)
            $operations.Add("move:$source->$destination") | Out-Null
            [System.IO.Directory]::Move($source, $destination)
        }.GetNewClosure()
        $cleanupBackup = {
            param($native, $backup)
            $operations.Add("cleanup:$backup") | Out-Null
            Remove-Item -LiteralPath (Join-Path $backup 'marker.txt') -Force
            throw 'injected partial backup cleanup failure'
        }.GetNewClosure()

        $publication = & $nativeBuildModule {
            param($native, $staged, $artifact, $move, $cleanup)
            $cleanupWarnings = @()
            $successOutput = @(
                Publish-RollbackSafeArtifact `
                    -NativeRoot $native `
                    -StagedArtifactRoot $staged `
                    -ArtifactRoot $artifact `
                    -MoveDirectory $move `
                    -CleanupBackup $cleanup `
                    -ValidateArtifact {
                        param($path)
                        [pscustomobject]@{
                            ArtifactRoot = $path
                            Marker = Get-Content -LiteralPath (Join-Path $path 'marker.txt') -Raw
                        }
                    } `
                    -WarningVariable cleanupWarnings `
                    -WarningAction SilentlyContinue
            )
            [pscustomobject]@{
                SuccessOutput = $successOutput
                WarningText = $cleanupWarnings -join [Environment]::NewLine
            }
        } $nativeRoot $stagedRoot $artifactRoot $moveDirectory $cleanupBackup

        Assert-Equal 1 $publication.SuccessOutput.Count 'Backup cleanup contaminated the successful publication output.'
        Assert-Equal $artifactRoot $publication.SuccessOutput[0].ArtifactRoot 'Publication did not return the validated replacement.'
        Assert-Equal 'validated-replacement' $publication.SuccessOutput[0].Marker 'The validated replacement was rolled back after commit.'
        Assert-Equal 'validated-replacement' ([System.IO.File]::ReadAllText((Join-Path $artifactRoot 'marker.txt'))) 'The validated replacement is not installed.'
        Assert-Equal 2 @($operations | Where-Object { $_ -like 'move:*' }).Count 'Publication tried to restore the damaged backup.'
        Assert-Equal 1 @(Get-ChildItem -LiteralPath $nativeRoot -Directory | Where-Object { $_.Name -like 'backup-*' }).Count 'Partial backup remnants were not retained for later cleanup.'
        if ($publication.WarningText -notmatch 'backup cleanup failed' -or
            $publication.WarningText -notmatch 'injected partial backup cleanup failure') {
            throw "Publication did not emit the required cleanup warning: '$($publication.WarningText)'."
        }
    }

    Invoke-Test 'cache publication restores the previous artifact when the staged move fails' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $nativeRoot = Join-Path $testRoot 'move-failure-cache'
        $artifactRoot = Join-Path $nativeRoot 'artifact'
        $stagedRoot = Join-Path $nativeRoot 'staged'
        New-Item -ItemType Directory -Path $artifactRoot, $stagedRoot -Force | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $artifactRoot 'marker.txt'), 'previous-validated')
        [System.IO.File]::WriteAllText((Join-Path $stagedRoot 'marker.txt'), 'replacement')
        $moveCount = 0
        $moveDirectory = {
            param($source, $destination)
            $script:moveCount++
            if ($script:moveCount -eq 2) {
                throw 'injected staged move failure'
            }
            [System.IO.Directory]::Move($source, $destination)
        }
        $script:moveCount = 0

        Assert-Throws {
            & $nativeBuildModule {
                param($native, $staged, $artifact, $move)
                Publish-RollbackSafeArtifact `
                    -NativeRoot $native `
                    -StagedArtifactRoot $staged `
                    -ArtifactRoot $artifact `
                    -MoveDirectory $move `
                    -ValidateArtifact { param($path) Get-Content -LiteralPath (Join-Path $path 'marker.txt') -Raw }
            } $nativeRoot $stagedRoot $artifactRoot $moveDirectory
        } 'injected staged move failure'

        Assert-Equal 'previous-validated' ([System.IO.File]::ReadAllText((Join-Path $artifactRoot 'marker.txt'))) 'The previous cache was not restored after the move failure.'
        Assert-Equal 0 @(Get-ChildItem -LiteralPath $nativeRoot -Directory | Where-Object { $_.Name -like 'backup-*' }).Count 'A rollback backup was left behind.'
    }

    Invoke-Test 'cache publication restores the previous artifact when replacement validation fails' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $nativeRoot = Join-Path $testRoot 'validation-failure-cache'
        $artifactRoot = Join-Path $nativeRoot 'artifact'
        $stagedRoot = Join-Path $nativeRoot 'staged'
        New-Item -ItemType Directory -Path $artifactRoot, $stagedRoot -Force | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $artifactRoot 'marker.txt'), 'previous-validated')
        [System.IO.File]::WriteAllText((Join-Path $stagedRoot 'marker.txt'), 'replacement')
        $moveDirectory = {
            param($source, $destination)
            [System.IO.Directory]::Move($source, $destination)
        }

        Assert-Throws {
            & $nativeBuildModule {
                param($native, $staged, $artifact, $move)
                Publish-RollbackSafeArtifact `
                    -NativeRoot $native `
                    -StagedArtifactRoot $staged `
                    -ArtifactRoot $artifact `
                    -MoveDirectory $move `
                    -ValidateArtifact { throw 'injected replacement validation failure' }
            } $nativeRoot $stagedRoot $artifactRoot $moveDirectory
        } 'injected replacement validation failure'

        Assert-Equal 'previous-validated' ([System.IO.File]::ReadAllText((Join-Path $artifactRoot 'marker.txt'))) 'The previous cache was not restored after validation failed.'
        Assert-Equal 0 @(Get-ChildItem -LiteralPath $nativeRoot -Directory | Where-Object { $_.Name -like 'backup-*' }).Count 'A rollback backup was left behind.'
    }

    Invoke-Test 'the native-build documentation states the reproducible Windows contract' {
        $readmePath = Join-Path $scriptsRoot 'README.md'
        $readme = [System.IO.File]::ReadAllText($readmePath)
        foreach ($requiredText in @(
            '17.14.37516.0',
            '17.14.51.32402',
            '14.44.35207',
            '14.44.35228.0',
            'x86_64-pc-windows-msvc',
            '.native/t',
            'openssl-sys-0123456789abcdef',
            'libdefault-lib-cipher_aes_cbc_hmac_sha256_etm_hw.obj',
            'less than 260',
            'shorter absolute `--target-dir`',
            'before `cargo clean` or the requested Cargo command',
            'CFLAGS=/Z7',
            'CXXFLAGS=/Z7',
            'lock-protected',
            'rollback-safe',
            'backup cleanup is best-effort',
            'cleanup failure emits a warning',
            'backup remnants for later cleanup'
        )) {
            if ($readme -notmatch [regex]::Escape($requiredText)) {
                throw "The native-build documentation is missing '$requiredText'."
            }
        }
        if ($readme -match '(?i)atomic(?:ally)?' -or
            $readme -match '(?i)suppress(?:es|ed|ing)?[^\r\n]*LNK4099' -or
            $readme -match '/IGNORE:4099') {
            throw 'The native-build documentation makes a stale atomicity or linker-suppression claim.'
        }
    }
}
finally {
    $resolvedTemp = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
    if (-not $resolvedTestRoot.StartsWith($resolvedTemp, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to remove test directory outside the system temp directory: $resolvedTestRoot"
    }
    Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
}

Write-Host "$script:passed Windows native build tests passed."
