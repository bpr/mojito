# `CompilationTarget`'s string-field predicates compare the target's
# `triple`, `triple_arch`, `arch`, and `os` fields. Only answers every
# target Mojito accepts (x86-64 Linux) shares print, so the host CPU name
# `_arch()` returns is not printed.
from std.sys import (
    CompilationTarget,
    is_amd_gpu,
    is_apple_gpu,
    is_gpu,
    is_nvidia_gpu,
)
from std.sys.info import _is_amd_cdna, _is_amd_rdna


def on_linux(os: String) -> Bool:
    return os == "linux"


def main():
    print(CompilationTarget.is_x86(), CompilationTarget.is_linux())
    print(CompilationTarget._os())
    print(
        CompilationTarget.is_macos(),
        CompilationTarget.is_arm(),
        CompilationTarget.is_riscv(),
    )
    print(CompilationTarget._unknown._os(), CompilationTarget._unknown.is_x86())
    print(CompilationTarget.is_triple["x86_64-unknown-linux-gnu"]())
    print(CompilationTarget.default_compile_options() == "")
    print(
        CompilationTarget.has_neon(),
        CompilationTarget.is_apple_silicon(),
        CompilationTarget.has_riscv_extension["m"](),
    )
    print(
        CompilationTarget.current().is_cpu(),
        CompilationTarget.current().is_gpu(),
        CompilationTarget._unknown().is_cpu(),
    )
    print(is_gpu(), is_nvidia_gpu(), is_amd_gpu(), is_apple_gpu())
    print(is_nvidia_gpu["sm_80"](), is_amd_gpu["gfx942"]())
    print(_is_amd_rdna(), _is_amd_cdna())
    comptime if CompilationTarget.is_linux():
        print("linux branch")
    var os = CompilationTarget._os()
    print(os in ["darwin", "macosx"], on_linux(os))
