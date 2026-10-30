# A bracketed call leaves a later struct-typed parameter to its default,
# whose own infer-only binder the default's type solves: `f[Int]()` and
# `g[3]()` over `target: CompilationTarget = CompilationTarget.current()`
# bind `target` to the compilation's target, as at the pin.
from std.sys import CompilationTarget, simd_byte_width


def f[
    T: AnyType, target: CompilationTarget = CompilationTarget.current()
]() -> Int:
    return simd_byte_width[target]()


def g[n: Int, target: CompilationTarget = CompilationTarget.current()]() -> Int:
    return simd_byte_width[target]() + n


def main():
    print(f[Int]() >= 16, f[T=Int]() == f[Int]())
    print(g[3]() - f[Int]())
# stdout: True True
# stdout: 3
