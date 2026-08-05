$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$script:BuildContract = [pscustomobject]@{
    SchemaVersion = 1
    RecipeRevision = 1
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
    Library = 'lib/libsodium.lib'
    CacheDirectory = 'libsodium-1.0.20-msvc-static-md-x64'
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
        $receipt = Get-Content -LiteralPath $ReceiptPath -Raw | ConvertFrom-Json
    }
    catch {
        throw "The libsodium build receipt is not valid JSON: $($_.Exception.Message)"
    }

    $contract = Get-WindowsNativeBuildContract
    $expected = [ordered]@{
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
        machine = 'x64'
        crt_default_library = 'MSVCRT'
    }

    foreach ($entry in $expected.GetEnumerator()) {
        $actual = Get-ReceiptValue -Receipt $receipt -Name $entry.Key
        if ($actual.ToString() -cne $entry.Value.ToString()) {
            throw "The libsodium build receipt field '$($entry.Key)' is stale or incompatible."
        }
    }

    foreach ($versionField in @('msbuild_version', 'link_version')) {
        $version = (Get-ReceiptValue -Receipt $receipt -Name $versionField).ToString()
        if ($version -notmatch '^\d+(?:\.\d+)+$') {
            throw "The libsodium build receipt field '$versionField' is invalid."
        }
    }

    $expectedLibraryHash = (Get-ReceiptValue -Receipt $receipt -Name 'library_sha256').ToString()
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

    $output = @(
        & $VsWherePath -latest -version '[17.0,18.0)' -products '*' -requires Microsoft.Component.MSBuild Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    )
    if ($LASTEXITCODE -ne 0) {
        throw "vswhere.exe failed while locating Visual Studio 2022 Build Tools (exit $LASTEXITCODE)."
    }
    $installationPath = $output | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Container) } | Select-Object -First 1
    if (-not $installationPath) {
        throw 'Visual Studio 2022 Build Tools with MSBuild and the x64 MSVC toolchain were not found.'
    }
    return [System.IO.Path]::GetFullPath($installationPath)
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

    $msvcToolsRoot = Join-Path $installation 'VC\Tools\MSVC'
    $toolVersions = @(Get-ChildItem -LiteralPath $msvcToolsRoot -Directory -ErrorAction SilentlyContinue |
        Where-Object {
            $parsed = $null
            [version]::TryParse($_.Name, [ref] $parsed) -and
                (Test-Path -LiteralPath (Join-Path $_.FullName 'bin\Hostx64\x64\link.exe') -PathType Leaf)
        } |
        Sort-Object { [version] $_.Name } -Descending)
    if ($toolVersions.Count -eq 0) {
        throw 'The selected Visual Studio 2022 installation has no x64 MSVC linker.'
    }
    $link = Join-Path $toolVersions[0].FullName 'bin\Hostx64\x64\link.exe'

    return [pscustomobject]@{
        InstallationPath = $installation
        DeveloperCommand = $developerCommand
        MsBuildPath = $msbuild
        LinkPath = $link
        V143ToolsetPath = $v143Toolset
        VcToolsInstallDirectory = $toolVersions[0].FullName
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
        [Parameter(Mandatory = $true)] [string] $DisplayName
    )

    $output = @(& $ExecutablePath @Arguments 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "$DisplayName version inspection failed (exit $LASTEXITCODE)."
    }
    foreach ($line in $output) {
        $match = [regex]::Match($line.ToString(), '(?<!\d)(\d+(?:\.\d+)+)(?!\d)')
        if ($match.Success) {
            return $match.Groups[1].Value
        }
    }
    throw "$DisplayName did not report a parseable version."
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
        [Parameter(Mandatory = $true)] [string] $LinkVersion
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
        library = $contract.Library
        library_sha256 = $libraryHash
        machine = 'x64'
        crt_default_library = 'MSVCRT'
        msbuild_version = $MsBuildVersion
        link_version = $LinkVersion
    }
}

function Get-ValidatedProvisionedArtifact {
    param(
        [Parameter(Mandatory = $true)] [string] $ArtifactRoot,
        [Parameter(Mandatory = $true)] [string] $LinkPath
    )

    $contract = Get-WindowsNativeBuildContract
    $libraryPath = Join-Path $ArtifactRoot ($contract.Library.Replace('/', '\'))
    $receiptPath = Join-Path $ArtifactRoot 'receipt.json'
    $receipt = Assert-LibsodiumBuildReceipt -ReceiptPath $receiptPath -LibraryPath $libraryPath
    $inspection = Invoke-LibsodiumArchiveInspection -LinkPath $LinkPath -LibraryPath $libraryPath
    if ($receipt.link_version -cne $inspection.LinkVersion) {
        throw 'The cached libsodium receipt was produced with a different MSVC linker version.'
    }

    return [pscustomobject]@{
        ArtifactRoot = $ArtifactRoot
        LibraryPath = $libraryPath
        LibraryDirectory = Split-Path -Parent $libraryPath
        ReceiptPath = $receiptPath
    }
}

function New-ProvisionedLibsodiumArtifact {
    param(
        [Parameter(Mandatory = $true)] [string] $NativeRoot,
        [Parameter(Mandatory = $true)] [string] $ArchivePath,
        [Parameter(Mandatory = $true)] [string] $MsBuildPath,
        [Parameter(Mandatory = $true)] [string] $LinkPath,
        [Parameter(Mandatory = $true)] [string] $ArtifactRoot
    )

    $contract = Get-WindowsNativeBuildContract
    $workRoot = Join-Path $NativeRoot 'w'
    $artifactStage = Join-Path $NativeRoot 's'
    $published = $false

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

        $solutionPath = Join-Path $sourceRoot ($contract.Solution.Replace('/', '\'))
        if (-not (Test-Path -LiteralPath $solutionPath -PathType Leaf)) {
            throw "The expected libsodium VS2022 solution is missing: $solutionPath"
        }

        $msbuildLog = Join-Path $workRoot 'msbuild.log'
        & $MsBuildPath $solutionPath /nologo /m /t:Rebuild "/p:Configuration=$($contract.Configuration)" "/p:Platform=$($contract.Platform)" "/p:PlatformToolset=$($contract.PlatformToolset)" /verbosity:minimal /fileLogger "/fileLoggerParameters:LogFile=$msbuildLog;Verbosity=normal"
        if ($LASTEXITCODE -ne 0) {
            $msbuildExitCode = $LASTEXITCODE
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

        $receipt = New-LibsodiumBuildReceipt -LibraryPath $stageLibrary -MsBuildVersion $msbuildVersion -LinkVersion $inspection.LinkVersion
        $receiptPath = Join-Path $artifactStage 'receipt.json'
        $receiptJson = $receipt | ConvertTo-Json -Depth 3
        [System.IO.File]::WriteAllText($receiptPath, $receiptJson + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))

        Assert-LibsodiumBuildReceipt -ReceiptPath $receiptPath -LibraryPath $stageLibrary | Out-Null
        Invoke-LibsodiumArchiveInspection -LinkPath $LinkPath -LibraryPath $stageLibrary | Out-Null

        if (Test-Path -LiteralPath $ArtifactRoot) {
            Remove-NativeTree -NativeRoot $NativeRoot -Path $ArtifactRoot
        }
        [System.IO.Directory]::Move($artifactStage, $ArtifactRoot)
        $published = $true

        return Get-ValidatedProvisionedArtifact -ArtifactRoot $ArtifactRoot -LinkPath $LinkPath
    }
    catch {
        if ($published -and (Test-Path -LiteralPath $ArtifactRoot)) {
            Remove-NativeTree -NativeRoot $NativeRoot -Path $ArtifactRoot
        }
        throw
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
        [Parameter(Mandatory = $true)] [string] $LinkPath
    )

    $contract = Get-WindowsNativeBuildContract
    $artifactRoot = Join-Path $NativeRoot $contract.CacheDirectory
    if (Test-Path -LiteralPath $artifactRoot) {
        try {
            return Get-ValidatedProvisionedArtifact -ArtifactRoot $artifactRoot -LinkPath $LinkPath
        }
        catch {
            Write-Warning "Rejecting cached libsodium artifact: $($_.Exception.Message)"
        }
    }

    $archivePath = Get-VerifiedLibsodiumSourceArchive -NativeRoot $NativeRoot
    return New-ProvisionedLibsodiumArtifact -NativeRoot $NativeRoot -ArchivePath $archivePath -MsBuildPath $MsBuildPath -LinkPath $LinkPath -ArtifactRoot $artifactRoot
}

function Get-CargoTargetDirectoryArgument {
    param(
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    for ($index = 0; $index -lt $CargoArguments.Count; $index++) {
        if ($CargoArguments[$index] -eq '--target-dir') {
            if ($index + 1 -ge $CargoArguments.Count) {
                throw 'Cargo --target-dir is missing its path argument.'
            }
            return $CargoArguments[$index + 1]
        }
        if ($CargoArguments[$index].StartsWith('--target-dir=', [System.StringComparison]::Ordinal)) {
            return $CargoArguments[$index].Substring('--target-dir='.Length)
        }
    }
    return $null
}

function Get-CargoTargetRoot {
    param(
        [Parameter(Mandatory = $true)] [string] $ManifestRoot,
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $configuredTarget = Get-CargoTargetDirectoryArgument -CargoArguments $CargoArguments
    if (-not $configuredTarget) {
        $configuredTarget = $env:CARGO_TARGET_DIR
    }
    if (-not $configuredTarget) {
        return Join-Path $ManifestRoot 'target'
    }
    if ([System.IO.Path]::IsPathRooted($configuredTarget)) {
        return [System.IO.Path]::GetFullPath($configuredTarget)
    }
    return [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $configuredTarget))
}

function Assert-CargoUsedProvisionedLibsodium {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)] [string] $TargetRoot,
        [Parameter(Mandatory = $true)] [string] $LibraryDirectory,
        [Parameter(Mandatory = $true)] [datetime] $NotBeforeUtc
    )

    if (-not (Test-Path -LiteralPath $TargetRoot -PathType Container)) {
        throw 'Cargo did not create the expected target directory.'
    }

    $patterns = @(
        (Join-Path $TargetRoot '*\build\libsodium-sys-stable-*\output'),
        (Join-Path $TargetRoot '*\*\build\libsodium-sys-stable-*\output')
    )
    $outputFiles = @($patterns |
        ForEach-Object { Get-ChildItem -Path $_ -File -ErrorAction SilentlyContinue } |
        Sort-Object FullName -Unique |
        Where-Object { $_.LastWriteTimeUtc -ge $NotBeforeUtc.AddSeconds(-5) })
    if ($outputFiles.Count -eq 0) {
        throw 'Cargo did not produce a fresh libsodium-sys-stable build-script output.'
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
        if ($argument -eq '--release') {
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

function Invoke-LibsodiumCargoClean {
    param(
        [Parameter(Mandatory = $true)] [string] $CargoPath,
        [Parameter(Mandatory = $true)] [string] $ManifestRoot,
        [Parameter(Mandatory = $true)] [string[]] $CargoArguments
    )

    $cleanArguments = @(
        'clean',
        '--manifest-path',
        (Join-Path $ManifestRoot 'Cargo.toml'),
        '-p',
        'libsodium-sys-stable'
    )
    $targetDirectory = Get-CargoTargetDirectoryArgument -CargoArguments $CargoArguments
    if ($targetDirectory) {
        $cleanArguments += @('--target-dir', $targetDirectory)
    }
    $cleanArguments += @(Get-CargoCleanProfileArguments -CargoArguments $CargoArguments)

    $cleanExitCode = Invoke-NativeCommandWithOutput -ExecutablePath $CargoPath -Arguments $cleanArguments
    if ($cleanExitCode -ne 0) {
        throw "Unable to clear cached libsodium-sys-stable artifacts (cargo clean exit $cleanExitCode)."
    }
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

        $artifact = Get-ProvisionedLibsodiumArtifact -NativeRoot $nativeRoot -MsBuildPath $toolchain.MsBuildPath -LinkPath $toolchain.LinkPath
        Write-Host "Verified libsodium $((Get-WindowsNativeBuildContract).LibsodiumVersion) StaticRelease x64 /MD artifact."

        if ($ProvisionOnly) {
            return 0
        }

        Assert-PortableOpenSslBuildTools
        $cargo = Get-Command cargo.exe -ErrorAction SilentlyContinue
        if ($null -eq $cargo) {
            throw 'cargo.exe was not found on PATH.'
        }

        $env:SODIUM_LIB_DIR = $artifact.LibraryDirectory
        $env:DESKTOP_LIBSODIUM_RECEIPT = $artifact.ReceiptPath
        $env:DESKTOP_MSVC_LINK = $toolchain.LinkPath
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

        $targetRoot = Get-CargoTargetRoot -ManifestRoot $manifestRootFull -CargoArguments $CargoArguments
        Invoke-LibsodiumCargoClean -CargoPath $cargo.Source -ManifestRoot $manifestRootFull -CargoArguments $CargoArguments
        $cargoStartedUtc = (Get-Date).ToUniversalTime()
        $cargoExitCode = Invoke-NativeCommandWithOutput -ExecutablePath $cargo.Source -Arguments $CargoArguments
        if ($cargoExitCode -eq 0) {
            Assert-CargoUsedProvisionedLibsodium -TargetRoot $targetRoot -LibraryDirectory $artifact.LibraryDirectory -NotBeforeUtc $cargoStartedUtc
        }
        return $cargoExitCode
    }
    finally {
        $lock.Dispose()
    }
}

Export-ModuleMember -Function @(
    'Assert-CargoUsedProvisionedLibsodium',
    'Assert-LibsodiumArchiveInspection',
    'Assert-LibsodiumBuildReceipt',
    'Convert-LibsodiumReleasePropertyToDynamicCrt',
    'Get-VisualStudio2022Toolchain',
    'Get-WindowsNativeBuildContract',
    'Invoke-WindowsNativeBuild'
)
