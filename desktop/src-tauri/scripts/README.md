# Windows native build

Rust `1.88.0` is the minimum supported compiler. Repository builds use the exact Rust `1.97.1`
toolchain declared in the root `rust-toolchain.toml`.

Run `windows-native-build.ps1` from the repository root. The wrapper accepts only this native-build
recipe:

- Visual Studio installation version `17.14.37516.0`
- MSBuild version `17.14.51.32402`
- VC tools directory version `14.44.35207`
- linker version `14.44.35228.0`

`vswhere` searches only the Visual Studio 17.14 line. The discovered installation metadata and all
three executable/tool-directory versions must match exactly; a Visual Studio update requires a
deliberate recipe revision. VsDevCmd, MSBuild, the x64 linker, and the v143 platform-toolset marker
must come from that one installation.

The wrapper downloads the official `libsodium-1.0.20.tar.gz` source asset into ignored
`desktop/src-tauri/.native` and requires SHA-256
`ebb65ef6ca439333c2bb41a0c1990587288da07f6c7fd07cb3a18cc18d30ce19` before extraction. Native
provisioning runs under the project-local lock. Before MSBuild starts, inherited `CL`, `_CL_`,
`LINK`, `_LINK_`, `ForceImportBeforeCppTargets`, and `ForceImportAfterCppTargets` values are
removed. MSBuild also receives `-noAutoResponse`, `ImportDirectoryBuildProps=false`, and
`ImportDirectoryBuildTargets=false`.

Provisioning extracts into a short staging path, changes the one verified `MultiThreaded` property
to `MultiThreadedDLL`, changes the one verified release debug-information property from
`ProgramDatabase` to `OldStyle`, and builds `StaticRelease|x64` with toolset `v143`. The result is
inspected with `link /dump /directives` and `/headers`. Every object must be x64. The archive must
request `/DEFAULTLIB:MSVCRT` and must not request `LIBCMT`, `LIBCMTD`, or `MSVCRTD`. A
`/FAILIFMISMATCH:RuntimeLibrary` directive is optional; when present, its value must be
`MD_DynamicRelease`.

The staged static library and `receipt.json` replace the cache at
`.native/libsodium-1.0.20-msvc-static-md-x64` using a lock-protected, rollback-safe sequence. The
previous cache remains as a backup until the replacement has moved into place and passed receipt,
hash, architecture, CRT, and tool-version validation. A move or validation failure restores that
backup. After validation commits the replacement, backup cleanup is best-effort. A cleanup failure emits a warning
and leaves backup remnants for later cleanup. It does not roll back the validated replacement. No libsodium DLL is
built or packaged.

Every requested Cargo command targets `x86_64-pc-windows-msvc` explicitly. When the caller omits
`--target-dir`, the wrapper intentionally selects the short `.native/t`; output is therefore under
`.native/t/x86_64-pc-windows-msvc/<profile>`. An explicit caller `--target-dir` is preserved and
resolved exactly. The selective `cargo clean -p libsodium-sys-stable`, requested command, and
post-build attestation all use the same target, target directory, and profile. Both `--release` and
`-r` select the `release` profile.

Before `cargo clean` or the requested Cargo command, the wrapper checks the resolved target root
against the vendored OpenSSL 3.6.3 path budget. The deterministic probe uses the pinned target and
profile plus
`build/openssl-sys-0123456789abcdef/out/openssl-build/build/src/providers/implementations/ciphers/libdefault-lib-cipher_aes_cbc_hmac_sha256_etm_hw.obj`.
Its full path must be less than 260 characters. This check also applies to caller-supplied target
directories; an over-budget path fails before Cargo starts and asks for a
shorter absolute `--target-dir`.

The wrapper rejects `cargo rustc`, `cargo bench`, Cargo `--config`, non-x86_64 Windows targets,
linker codegen overrides, and `+crt-static`. It replaces ambient Cargo target, target-directory,
linker, and Rust flag settings. `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER` names the approved linker,
`CARGO_ENCODED_RUSTFLAGS=-Ctarget-feature=-crt-static`, and ambient `RUSTFLAGS` plus target
rustflags are removed. Vendored OpenSSL 3.6.3 builds receive controlled `CFLAGS=/Z7` and
`CXXFLAGS=/Z7`, overriding ambient values so debug information is embedded in objects instead of
requiring `ossl_static.pdb`. No linker-warning suppression is part of the recipe.

Example:

```powershell
& .\desktop\src-tauri\scripts\windows-native-build.ps1 -CargoArguments @(
  'test', '--manifest-path', '.\desktop\src-tauri\Cargo.toml', '--test', 'storage', '-r', '--', '--test-threads=1'
)
```

Use `-ProvisionOnly` to build or validate the project-local artifact without invoking Cargo.
Vendored OpenSSL Cargo commands additionally require portable Strawberry Perl and NASM on `PATH`;
if `PERL` names the Strawberry `perl.exe`, the wrapper prepends its directory to `PATH`.

Windows package builds fail closed unless `SODIUM_LIB_DIR`, `DESKTOP_LIBSODIUM_RECEIPT`, and the
selected linker identify the exact project-local artifact and approved toolchain. The Rust build
script independently validates the closed receipt schema, source and library hashes, selected
paths, x64 machine headers, CRT directives, and inspecting linker version.
