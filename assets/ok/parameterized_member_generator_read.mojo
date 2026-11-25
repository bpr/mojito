# A parameterized `comptime` member read unapplied is the generator its
# declaration is, upstream's `#kgen.gen`, whether its body is a type or a
# value: `comptime g = S.B` binds the generator, and `g[3]` applies it as
# `S.B[3]` would. A type spelled by an `__mlir_attr` body is such a type
# member too, and over a pack still open the elaborator applies it. The
# generator binds the member's whole list: a type pack its body spreads
# (`Tuple[*Ts]`), and an origin binder with the `mut=` binder it declares
# (`List[Int].IteratorType[origin_of(xs)]`), as a module alias's does.
from std.builtin.variadics import _MLIR


@fieldwise_init
struct Box[n: Int](Copyable):
    var v: Int


struct S:
    comptime B[n: Int] = Box[n]
    comptime P[*Ts: AnyType] = Tuple[*Ts]


comptime Tup[*Ts: AnyType] = Tuple[*Ts]
comptime IntSpan[o: ImmOrigin] = Span[Int, o]


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
    comptime p = S.P
    comptime U = p[Int, Bool]
    var t: U = (1, True)
    print(t[0], t[1])
    comptime h = Tup
    comptime V = h[Bool, Int]
    var w: V = (False, 2)
    print(w[0], w[1])
    var xs: List[Int] = [7, 8, 9]
    comptime I = List[Int].IteratorType
    comptime J = I[origin_of(xs)]
    comptime assert reflect[J].name() == reflect[
        List[Int].IteratorType[origin_of(xs)]
    ].name()
    comptime s = IntSpan
    var span: s[origin_of(xs)] = Span(xs)
    print(span[1], len(xs))
