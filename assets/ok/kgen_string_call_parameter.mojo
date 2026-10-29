# A call returning a `!kgen.string` is a compile-time application where a
# parameter or an op's attribute takes one: a struct's parameter, a
# `StringLiteral`'s, and `pop.call_llvm_intrinsic`'s `intrin`.
from std.collections.string.string_span import _get_kgen_string


def name() -> __mlir_type.`!kgen.string`:
    return "abc".value


struct G[n: __mlir_type.`!kgen.string`]:
    @staticmethod
    def x() -> Int:
        return StaticString(Self.n).byte_length()


def main():
    print(G[name()].x())
    print(StringLiteral[_get_kgen_string["ab", "c"]()]())
    var x: Int64 = 7
    print(
        __mlir_op.`pop.call_llvm_intrinsic`[
            intrin=_get_kgen_string["llvm.ctpop"](),
            _type=Int64,
            hasSideEffects=False.__mlir_i1__(),
        ](x)
    )
