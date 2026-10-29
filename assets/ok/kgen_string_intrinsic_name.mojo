# `_get_kgen_string` concatenates compile-time `StaticString` values into
# the `!kgen.string` an LLVM intrinsic call names, as `llvm_intrinsic` does
# for its `intrin`.
from std.collections.string.string_span import _get_kgen_string
from std.sys import llvm_intrinsic


def count[name: StaticString](x: Int64) -> Int64:
    comptime k = _get_kgen_string["llvm.", name]()
    return __mlir_op.`pop.call_llvm_intrinsic`[
        intrin=k, _type=Int64, hasSideEffects=False.__mlir_i1__()
    ](x)


def main():
    print(count["ctpop"](255))
    print(count["bitreverse"](1) < 0)
    print(llvm_intrinsic["llvm.ctpop", Int64, has_side_effect=False](Int64(7)))
