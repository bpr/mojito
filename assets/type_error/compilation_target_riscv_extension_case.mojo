# expect: RISC-V extension names are lowercase, got: M
from std.sys import CompilationTarget


def main():
    print(CompilationTarget.has_riscv_extension["M"]())
