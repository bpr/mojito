# A parameterized `comptime` member read unapplied is the generator its
# declaration is, upstream's `#kgen.gen`, whether its body is a type or a
# value: `comptime g = S.B` binds the generator, and `g[3]` applies it as
# `S.B[3]` would. A type spelled by an `__mlir_attr` body is such a type
# member too, and over a pack still open the elaborator applies it.
from std.builtin.variadics import _MLIR


@fieldwise_init
struct Box[n: Int](Copyable):
    var v: Int


struct S:
    comptime B[n: Int] = Box[n]


struct RawList[Trait: type_of(AnyType), //, values: _MLIR.KGENParamListType[Trait]]:
    comptime _mlir_type = _MLIR.KGENParamListType[Self.Trait]
    comptime get[idx: SIMDLength] = __mlir_attr[
        `#kgen.param_list.get<:`,
        Self._mlir_type,
        ` `,
        +Self.values,
        `, `,
        idx._mlir_value,
        `> : `,
        +Self.Trait,
    ]


def use[k: Int]():
    comptime g = S.B
    comptime T = g[k]
    print(T.n)


def show[*Ts: AnyType]():
    comptime g = RawList[Ts.values].get
    comptime T = g[1]
    comptime assert T == Bool
    comptime assert g[0] == Int
    print("asserted")


def main():
    use[5]()
    comptime g = S.B
    comptime T = g[3]
    var x = T(1)
    print(x.v, T.n)
    show[Int, Bool]()
