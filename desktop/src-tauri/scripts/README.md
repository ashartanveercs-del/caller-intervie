# Windows native build

Run `windows-native-build.ps1` from the repository root. The wrapper uses `vswhere` with version
range `[17.0,18.0)` to select one Visual Studio 2022 installation that has both MSBuild and x64
MSVC. VsDevCmd, MSBuild, the latest x64 linker, and the v143 platform-toolset marker are all
derived from that same installation. The wrapper then downloads the official
`libsodium-1.0.20.tar.gz` source asset into ignored `desktop/src-tauri/.native`. It requires SHA-256
`ebb65ef6ca439333c2bb41a0c1990587288da07f6c7fd07cb3a18cc18d30ce19` before extraction.

Provisioning extracts into a short, lock-protected staging path, verifies that
`builds/msvc/properties/ReleaseLIB.props` contains exactly one `MultiThreaded` runtime node, changes
that node to `MultiThreadedDLL`, and builds the VS2022 solution as `StaticRelease|x64` with toolset
`v143`. It then inspects `libsodium.lib` with MSVC `link /dump /directives` and `/headers`. The
archive is rejected unless every object is x64, `MSVCRT` is present, and neither `LIBCMT` nor
`LIBCMTD` is present.

Only a validated static library and `receipt.json` are published atomically to
`.native/libsodium-1.0.20-msvc-static-md-x64`. The receipt records the immutable source contract,
exact property replacement count, build configuration, tool versions, CRT directive, and library
SHA-256. Every wrapper invocation rechecks the receipt, artifact hash, architecture, and CRT
directives before invoking Cargo. No libsodium DLL is built or packaged.

Before the requested Cargo command, the wrapper runs package-scoped
`cargo clean -p libsodium-sys-stable`. This removes any bundled archive that a failed unsupported
direct Cargo attempt may have cached. After a successful command, the wrapper inspects the fresh
dependency build-script output and rejects it unless every libsodium native link-search directive
names the provisioned project-local directory.

No machine-specific path is tracked. Example:

```powershell
& .\desktop\src-tauri\scripts\windows-native-build.ps1 -CargoArguments @(
  'test', '--manifest-path', '.\desktop\src-tauri\Cargo.toml', '--test', 'storage', '--', '--test-threads=1'
)
```

Use `-ProvisionOnly` to build or validate the project-local artifact without invoking Cargo.
Vendored OpenSSL Cargo commands additionally require portable Strawberry Perl and NASM on `PATH`;
if `PERL` names the Strawberry `perl.exe`, the wrapper prepends its directory to `PATH`.

Windows MSVC package builds fail closed unless `SODIUM_LIB_DIR` and
`DESKTOP_LIBSODIUM_RECEIPT` select this exact provisioned artifact and receipt. They also require
the wrapper-selected linker to match `VCToolsInstallDir` beneath the selected version-17
`VSINSTALLDIR`. The build script independently hashes both the official source archive and
`libsodium.lib`, then runs that linker with `/dump /directives` and `/dump /headers`; receipt CRT
and machine claims are not trusted by themselves. The wrapper clears shared, pkg-config, and vcpkg
selection variables so `libsodium-sys-stable` cannot select its bundled `/MT(d)` archive.

The supported threat model assumes the tracked wrapper/build script and the local Visual Studio
installation are trusted. Manually fabricating environment variables or mutating both a cached
library and its receipt is not a supported build path; even then, the package build still enforces
the source checksum plus the actual archive's x64 and dynamic-CRT directives. The package build
script suppresses only missing-native-PDB warning `LNK4099`, and only on this package's binary,
cdylib, and test final links. It never suppresses `LNK4098` or blanket Rust linker diagnostics.
