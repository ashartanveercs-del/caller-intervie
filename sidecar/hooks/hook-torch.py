"""Keep only CPU inference files required by the frozen sidecar."""

from pathlib import Path

from PyInstaller.utils.hooks import collect_data_files, collect_dynamic_libs, collect_submodules


datas = collect_data_files(
    "torch",
    excludes=[
        "**/include/**",
        "**/test/**",
        "**/tests/**",
        "**/*.h",
        "**/*.hpp",
        "**/*.cuh",
        "**/*.lib",
        "**/*.pyi",
        "**/_inductor/**",
        "**/onnx/**",
        "**/ao/**",
    ],
)

# PyTorch's generic hook imports every submodule, including training and CUDA
# trees. CPU inference only needs the native CPU libraries reached by torch._C.
binaries = [
    (source, destination)
    for source, destination in collect_dynamic_libs("torch")
    if not any(
        marker in Path(source).name.lower()
        for marker in ("cuda", "cudnn", "cufft", "curand", "nvrtc", "nvtool")
    )
]

hiddenimports = ["torch._C", "torch.nn", "torch.nn.functional"]
# Transformers imports torch._dynamo during optional-integration discovery. Its
# loader enumerates this small package by string at import time.
hiddenimports += collect_submodules("torch._dynamo.polyfills")

# Torch configuration discovers source with inspect at import time. Keep source
# beside the archive for the modules PyInstaller traces without collecting every
# optional training or CUDA submodule.
module_collection_mode = "pyz+py"
