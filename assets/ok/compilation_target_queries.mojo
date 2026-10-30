# `CompilationTarget` answers its queries from the compilation target's
# `#kgen.target` attribute, which the elaborator supplies for
# `_current_target()`. Only answers every x86-64 target shares print:
# `sse2` is in the baseline, and the unknown target has no features and
# zero-width vectors.
from std.sys import CompilationTarget, simd_bit_width, simd_byte_width


def vector_words[target: CompilationTarget = CompilationTarget.current()]() -> Int:
    return simd_byte_width[target]() // 8


def main():
    print(CompilationTarget._has_feature["sse2"]())
    print(CompilationTarget.current._has_feature["sse2"]())
    print(CompilationTarget._unknown.has_avx2(), CompilationTarget._unknown.has_fma())
    print(
        simd_bit_width[CompilationTarget._unknown()](),
        simd_byte_width[CompilationTarget._unknown()](),
    )
    print(vector_words[CompilationTarget._unknown()]())
    print(simd_bit_width() >= 128, vector_words() >= 2)
    comptime if CompilationTarget.has_avx512f():
        print("vector width decided at compile time")
    else:
        print("vector width decided at compile time")
