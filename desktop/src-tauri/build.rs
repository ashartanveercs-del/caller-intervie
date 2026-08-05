use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    env,
    fmt::Write as _,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

const CACHE_DIRECTORY: &str = "libsodium-1.0.20-msvc-static-md-x64";
const SOURCE_URL: &str = "https://github.com/jedisct1/libsodium/releases/download/1.0.20-RELEASE/libsodium-1.0.20.tar.gz";
const SOURCE_SHA256: &str = "ebb65ef6ca439333c2bb41a0c1990587288da07f6c7fd07cb3a18cc18d30ce19";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildReceipt {
    schema_version: u32,
    recipe_revision: u32,
    libsodium_version: String,
    source_asset: String,
    source_url: String,
    source_sha256: String,
    solution: String,
    configuration: String,
    platform: String,
    platform_toolset: String,
    runtime_library: String,
    property_file: String,
    property_transform_count: u32,
    library: String,
    library_sha256: String,
    machine: String,
    crt_default_library: String,
    msbuild_version: String,
    link_version: String,
}

fn main() {
    for variable in [
        "SODIUM_LIB_DIR",
        "SODIUM_SHARED",
        "SODIUM_USE_PKG_CONFIG",
        "DESKTOP_LIBSODIUM_RECEIPT",
        "DESKTOP_MSVC_LINK",
        "VSINSTALLDIR",
        "VCToolsInstallDir",
        "VisualStudioVersion",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }

    if is_windows_msvc_target() {
        validate_windows_libsodium();

        println!("cargo:rustc-link-arg-bins=/IGNORE:4099");
        println!("cargo:rustc-link-arg-cdylib=/IGNORE:4099");
        println!("cargo:rustc-link-arg-tests=/IGNORE:4099");
    }

    tauri_build::build()
}

fn is_windows_msvc_target() -> bool {
    env::var("CARGO_CFG_TARGET_OS").is_ok_and(|value| value == "windows")
        && env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|value| value == "msvc")
}

fn validate_windows_libsodium() {
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("x86_64") {
        panic!("the provisioned Windows libsodium artifact supports only the x86_64 MSVC target");
    }
    if env::var_os("SODIUM_SHARED").is_some() {
        panic!("SODIUM_SHARED is incompatible with the required static libsodium artifact");
    }
    if env::var_os("SODIUM_USE_PKG_CONFIG").is_some() {
        panic!("SODIUM_USE_PKG_CONFIG is incompatible with the pinned libsodium artifact");
    }

    let manifest_root = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by Cargo"),
    );
    let artifact_root = manifest_root.join(".native").join(CACHE_DIRECTORY);
    let expected_library_directory = artifact_root.join("lib");
    let expected_library = expected_library_directory.join("libsodium.lib");
    let expected_receipt = artifact_root.join("receipt.json");
    let source_archive = manifest_root
        .join(".native")
        .join("libsodium-1.0.20.tar.gz");

    println!("cargo:rerun-if-changed={}", expected_library.display());
    println!("cargo:rerun-if-changed={}", expected_receipt.display());
    println!("cargo:rerun-if-changed={}", source_archive.display());

    let configured_library_directory = required_environment_path(
        "SODIUM_LIB_DIR",
        "run scripts/windows-native-build.ps1 so bundled libsodium cannot be selected",
    );
    require_same_canonical_path(
        &configured_library_directory,
        &expected_library_directory,
        "SODIUM_LIB_DIR does not select the project-local pinned artifact",
    );

    let configured_receipt = required_environment_path(
        "DESKTOP_LIBSODIUM_RECEIPT",
        "run scripts/windows-native-build.ps1 to validate and select the build receipt",
    );
    require_same_canonical_path(
        &configured_receipt,
        &expected_receipt,
        "DESKTOP_LIBSODIUM_RECEIPT does not select the project-local receipt",
    );

    let library_metadata = fs::metadata(&expected_library)
        .unwrap_or_else(|_| panic!("the provisioned libsodium.lib is missing"));
    if !library_metadata.is_file() || library_metadata.len() == 0 {
        panic!("the provisioned libsodium.lib is not a nonempty file");
    }
    if sha256_file(&source_archive) != SOURCE_SHA256 {
        panic!("the official libsodium source archive is missing or has the wrong SHA-256");
    }

    let receipt_bytes = fs::read(&expected_receipt)
        .unwrap_or_else(|_| panic!("the provisioned libsodium build receipt is missing"));
    if receipt_bytes.len() > 65_536 {
        panic!("the provisioned libsodium build receipt is unexpectedly large");
    }
    let receipt: BuildReceipt = serde_json::from_slice(&receipt_bytes).unwrap_or_else(|error| {
        panic!("the provisioned libsodium build receipt is invalid: {error}")
    });

    validate_receipt_contract(&receipt);
    let actual_library_sha256 = sha256_file(&expected_library);
    if receipt.library_sha256 != actual_library_sha256 {
        panic!("the provisioned libsodium.lib SHA-256 does not match its build receipt");
    }

    let link = validate_msvc_linker();
    inspect_libsodium_archive(&link, &expected_library, &receipt.link_version);
}

fn required_environment_path(variable: &str, help: &str) -> PathBuf {
    let value = env::var_os(variable).unwrap_or_else(|| panic!("{variable} is absent; {help}"));
    if value.is_empty() {
        panic!("{variable} is empty; {help}");
    }
    PathBuf::from(value)
}

fn require_same_canonical_path(configured: &Path, expected: &Path, message: &str) {
    let configured = configured
        .canonicalize()
        .unwrap_or_else(|_| panic!("{message}: the configured path is unavailable"));
    let expected = expected
        .canonicalize()
        .unwrap_or_else(|_| panic!("{message}: the expected path is unavailable"));
    if configured != expected {
        panic!("{message}");
    }
}

fn validate_msvc_linker() -> PathBuf {
    let visual_studio_version = env::var("VisualStudioVersion").unwrap_or_else(|_| {
        panic!("VisualStudioVersion is absent; use the Windows native wrapper")
    });
    if visual_studio_version
        .split('.')
        .next()
        .is_none_or(|major| major != "17")
    {
        panic!("the Windows native wrapper requires Visual Studio 2022 version 17.x");
    }

    let visual_studio_root = canonical_directory(
        &required_environment_path(
            "VSINSTALLDIR",
            "use the Windows native wrapper to select Visual Studio 2022",
        ),
        "VSINSTALLDIR does not name an available Visual Studio installation",
    );
    let vc_tools_root = canonical_directory(
        &required_environment_path(
            "VCToolsInstallDir",
            "use the Windows native wrapper to select the v143 MSVC tools",
        ),
        "VCToolsInstallDir does not name an available MSVC tool directory",
    );
    if !path_is_within(&vc_tools_root, &visual_studio_root) {
        panic!("VCToolsInstallDir is outside the selected Visual Studio 2022 installation");
    }

    let expected_link = vc_tools_root
        .join("bin")
        .join("Hostx64")
        .join("x64")
        .join("link.exe");
    let configured_link = required_environment_path(
        "DESKTOP_MSVC_LINK",
        "use the Windows native wrapper to select the inspected MSVC linker",
    );
    require_same_canonical_path(
        &configured_link,
        &expected_link,
        "DESKTOP_MSVC_LINK does not match the selected Visual Studio 2022 toolchain",
    );
    configured_link
        .canonicalize()
        .unwrap_or_else(|_| panic!("the selected x64 MSVC linker is unavailable"))
}

fn canonical_directory(path: &Path, message: &str) -> PathBuf {
    let canonical = path.canonicalize().unwrap_or_else(|_| panic!("{message}"));
    if !canonical.is_dir() {
        panic!("{message}");
    }
    canonical
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let root = root
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase();
    path.starts_with(&(root + "\\"))
}

fn inspect_libsodium_archive(link: &Path, library: &Path, receipt_link_version: &str) {
    let directives = run_link_dump(link, "/directives", library);
    let default_libraries: Vec<_> = directives
        .split_whitespace()
        .filter_map(|token| {
            let normalized = token.replace('"', "").to_ascii_uppercase();
            normalized.strip_prefix("/DEFAULTLIB:").map(str::to_owned)
        })
        .collect();
    if default_libraries
        .iter()
        .any(|library| library == "LIBCMT" || library == "LIBCMTD")
    {
        panic!("the provisioned libsodium.lib requests LIBCMT or LIBCMTD");
    }
    if !default_libraries.iter().any(|library| library == "MSVCRT") {
        panic!("the provisioned libsodium.lib does not request the release dynamic CRT MSVCRT");
    }

    let actual_link_version = directives
        .lines()
        .find_map(|line| {
            line.split_once("COFF/PE Dumper Version ")
                .map(|(_, value)| value.trim())
        })
        .unwrap_or_else(|| panic!("link.exe /dump did not report its version"));
    if actual_link_version != receipt_link_version {
        panic!("the build receipt linker version does not match the inspecting linker");
    }

    let headers = run_link_dump(link, "/headers", library);
    let machine_lines: Vec<_> = headers
        .lines()
        .map(str::trim)
        .filter(|line| line.to_ascii_lowercase().contains(" machine ("))
        .collect();
    if machine_lines.is_empty() {
        panic!("the provisioned libsodium.lib has no inspectable COFF machine headers");
    }
    if machine_lines
        .iter()
        .any(|line| !line.to_ascii_lowercase().ends_with("machine (x64)"))
    {
        panic!("the provisioned libsodium.lib contains a non-x64 object");
    }
}

fn run_link_dump(link: &Path, mode: &str, library: &Path) -> String {
    let output = Command::new(link)
        .args(["/dump", mode])
        .arg(library)
        .output()
        .unwrap_or_else(|error| panic!("unable to execute link.exe {mode}: {error}"));
    if !output.status.success() {
        panic!(
            "link.exe {mode} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    text
}

fn validate_receipt_contract(receipt: &BuildReceipt) {
    require_receipt_value("schema_version", receipt.schema_version, 1);
    require_receipt_value("recipe_revision", receipt.recipe_revision, 1);
    require_receipt_value(
        "libsodium_version",
        receipt.libsodium_version.as_str(),
        "1.0.20",
    );
    require_receipt_value(
        "source_asset",
        receipt.source_asset.as_str(),
        "libsodium-1.0.20.tar.gz",
    );
    require_receipt_value("source_url", receipt.source_url.as_str(), SOURCE_URL);
    require_receipt_value(
        "source_sha256",
        receipt.source_sha256.as_str(),
        SOURCE_SHA256,
    );
    require_receipt_value(
        "solution",
        receipt.solution.as_str(),
        "builds/msvc/vs2022/libsodium.sln",
    );
    require_receipt_value(
        "configuration",
        receipt.configuration.as_str(),
        "StaticRelease",
    );
    require_receipt_value("platform", receipt.platform.as_str(), "x64");
    require_receipt_value(
        "platform_toolset",
        receipt.platform_toolset.as_str(),
        "v143",
    );
    require_receipt_value(
        "runtime_library",
        receipt.runtime_library.as_str(),
        "MultiThreadedDLL",
    );
    require_receipt_value(
        "property_file",
        receipt.property_file.as_str(),
        "builds/msvc/properties/ReleaseLIB.props",
    );
    require_receipt_value(
        "property_transform_count",
        receipt.property_transform_count,
        1,
    );
    require_receipt_value("library", receipt.library.as_str(), "lib/libsodium.lib");
    require_receipt_value("machine", receipt.machine.as_str(), "x64");
    require_receipt_value(
        "crt_default_library",
        receipt.crt_default_library.as_str(),
        "MSVCRT",
    );

    if !is_version(&receipt.msbuild_version) {
        panic!("the libsodium build receipt field 'msbuild_version' is invalid");
    }
    if !is_version(&receipt.link_version) {
        panic!("the libsodium build receipt field 'link_version' is invalid");
    }
    if receipt.library_sha256.len() != 64
        || !receipt
            .library_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        panic!("the libsodium build receipt field 'library_sha256' is invalid");
    }
}

fn require_receipt_value<T>(field: &str, actual: T, expected: T)
where
    T: PartialEq,
{
    if actual != expected {
        panic!("the libsodium build receipt field '{field}' is stale or incompatible");
    }
}

fn is_version(value: &str) -> bool {
    let components: Vec<_> = value.split('.').collect();
    components.len() >= 2
        && components.iter().all(|component| {
            !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn sha256_file(path: &Path) -> String {
    let mut file = File::open(path)
        .unwrap_or_else(|_| panic!("unable to open a required native build artifact"));
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let read = file
            .read(&mut buffer)
            .unwrap_or_else(|_| panic!("unable to hash a required native build artifact"));
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let mut digest = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut digest, "{byte:02x}").expect("writing to a String cannot fail");
    }
    digest
}
