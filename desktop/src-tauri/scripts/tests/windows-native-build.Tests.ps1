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

    & $Test
    $script:passed++
    Write-Host "PASS: $Name"
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

    Invoke-Test 'directive inspection rejects LIBCMT even when MSVCRT is also present' {
        $directives = @'
   /DEFAULTLIB:"MSVCRT" /DEFAULTLIB:"LIBCMT"
'@
        $headers = '            8664 machine (x64)'

        Assert-Throws {
            Assert-LibsodiumArchiveInspection -DirectiveOutput $directives -HeaderOutput $headers
        } 'LIBCMT'
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
            library = $contract.Library
            library_sha256 = $libraryHash
            machine = 'x64'
            crt_default_library = 'MSVCRT'
            msbuild_version = '17.14.23.12345'
            link_version = '14.44.35228.0'
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
            library = $contract.Library
            library_sha256 = $libraryHash
            machine = 'x64'
            crt_default_library = 'MSVCRT'
            msbuild_version = '17.14.23.12345'
            link_version = '14.44.35228.0'
        }
        $receipt | ConvertTo-Json | Set-Content -LiteralPath $receiptPath -Encoding UTF8

        Assert-Throws {
            Assert-LibsodiumBuildReceipt -ReceiptPath $receiptPath -LibraryPath $libraryPath
        } 'source_sha256'
    }

    Invoke-Test 'the Rust build contract validates the receipt and suppresses only LNK4099 on final links' {
        $manifestRoot = Split-Path -Parent $scriptsRoot
        $buildScriptPath = Join-Path $manifestRoot 'build.rs'
        $cargoManifestPath = Join-Path $manifestRoot 'Cargo.toml'
        $buildScript = [System.IO.File]::ReadAllText($buildScriptPath)
        $cargoManifest = [System.IO.File]::ReadAllText($cargoManifestPath)

        foreach ($requiredText in @(
            'DESKTOP_LIBSODIUM_RECEIPT',
            'DESKTOP_MSVC_LINK',
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

        $ignoreCodes = @([regex]::Matches($buildScript, '/IGNORE:(\d+)') | ForEach-Object { $_.Groups[1].Value })
        Assert-Equal 3 $ignoreCodes.Count 'Unexpected number of linker warning suppressions.'
        foreach ($ignoreCode in $ignoreCodes) {
            Assert-Equal '4099' $ignoreCode 'An unsupported linker warning is suppressed.'
        }
        foreach ($scope in @('bins', 'cdylib', 'tests')) {
            if ($buildScript -notmatch [regex]::Escape("cargo:rustc-link-arg-$scope=/IGNORE:4099")) {
                throw "The LNK4099 suppression is missing the '$scope' final-link scope."
            }
        }
        if ($buildScript -match 'cargo:rustc-link-arg=/IGNORE' -or $buildScript -match 'linker_messages') {
            throw 'The linker warning suppression is broader than the package final-link scopes.'
        }
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

    Invoke-Test 'Visual Studio selection is constrained to version 17 and rejects a missing v143 toolset' {
        $moduleSource = [System.IO.File]::ReadAllText($modulePath)
        if ($moduleSource -notmatch [regex]::Escape("-version '[17.0,18.0)'")) {
            throw 'vswhere selection is not constrained to Visual Studio 2022 version 17.x.'
        }

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

    Invoke-Test 'Cargo native output validation rejects bundled fallback contamination' {
        $targetRoot = Join-Path $testRoot 'target'
        $buildRoot = Join-Path $targetRoot 'debug\build\libsodium-sys-stable-fixture'
        $outputPath = Join-Path $buildRoot 'output'
        $libraryDirectory = Join-Path $testRoot 'artifact\lib'
        New-Item -ItemType Directory -Path $buildRoot -Force | Out-Null
        New-Item -ItemType Directory -Path $libraryDirectory -Force | Out-Null
        $notBefore = (Get-Date).ToUniversalTime().AddSeconds(-5)

        [System.IO.File]::WriteAllText($outputPath, "cargo:rustc-link-search=native=$libraryDirectory`r`n")
        Assert-CargoUsedProvisionedLibsodium -TargetRoot $targetRoot -LibraryDirectory $libraryDirectory -NotBeforeUtc $notBefore

        $fallbackDirectory = Join-Path $testRoot 'bundled\out\installed\lib'
        [System.IO.File]::WriteAllText($outputPath, "cargo:rustc-link-search=native=$fallbackDirectory")
        Assert-Throws {
            Assert-CargoUsedProvisionedLibsodium -TargetRoot $targetRoot -LibraryDirectory $libraryDirectory -NotBeforeUtc $notBefore
        } 'provisioned libsodium'
    }

    Invoke-Test 'the wrapper cleans libsodium-sys before invoking Cargo' {
        $moduleSource = [System.IO.File]::ReadAllText($modulePath)
        $cleanIndex = $moduleSource.IndexOf("'clean'")
        $invokeIndex = $moduleSource.IndexOf('Invoke-NativeCommandWithOutput -ExecutablePath $cargo.Source -Arguments $CargoArguments')
        if ($cleanIndex -lt 0 -or $invokeIndex -lt 0 -or $cleanIndex -gt $invokeIndex) {
            throw 'The wrapper does not clean libsodium-sys-stable before the requested Cargo command.'
        }
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

    Invoke-Test 'native command forwarding tolerates successful stderr and returns only the exit code' {
        $nativeBuildModule = Get-Module | Where-Object { $_.Path -eq $modulePath } | Select-Object -First 1
        $exitCode = & $nativeBuildModule {
            param($executable, $arguments)
            Invoke-NativeCommandWithOutput -ExecutablePath $executable -Arguments $arguments
        } $env:ComSpec ([string[]] @('/d', '/s', '/c', 'echo fixture-stderr 1>&2'))

        Assert-Equal 0 $exitCode 'Successful native stderr contaminated the numeric return value.'
    }

    Invoke-Test 'the wrapper preserves Cargo output and its native exit code' {
        $moduleSource = [System.IO.File]::ReadAllText($modulePath)
        $invokeText = 'Invoke-NativeCommandWithOutput -ExecutablePath $cargo.Source -Arguments $CargoArguments'
        if ($moduleSource -notmatch [regex]::Escape($invokeText)) {
            throw 'Cargo stdout can contaminate the wrapper numeric return value.'
        }
        $captureText = '$cargoExitCode = ' + $invokeText
        if ($moduleSource.IndexOf($captureText) -lt 0) {
            throw 'The wrapper does not capture Cargo native exit status after invocation.'
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
