# The address of a place keeps the place's address space, as upstream's
# `Pointer.__init__(out self, *, ref[Self.origin, Self.address_space] to:
# Self.T)` and `__get_mvalue_as_litref` keep it: a `Pointer(to=x)` or
# `!lit.ref` over a `ref[_, a] x` parameter is in space `a`, so a struct
# declaring upstream's three `Pointer` initializers builds itself through
# its `ref ... to` form, and still selects the `_mlir_value=` ones; the
# struct's explicit origin binds the `!lit.ref` one's parameter type.
# Upstream prints `0`, `0`, `5`, `5`, `5`, `AddressSpace.GENERIC`, `True`,
# and `True`.
from std.memory import AddressSpace


struct P[
    mut: Bool,
    //,
    T: AnyType,
    origin: Origin[mut=mut],
    *,
    address_space: AddressSpace = .GENERIC,
](ImplicitlyCopyable):
    comptime _mlir_lit_ref = __mlir_type[
        `!lit.ref<`,
        Self.T,
        `, `,
        Self.origin._mlir_origin,
        `, `,
        Self.address_space._value._mlir_value,
        `>`,
    ]

    comptime _mlir_type = __mlir_type[
        `!kgen.pointer<`,
        Self.T,
        `, `,
        Self.address_space._value._mlir_value,
        `>`,
    ]

    var _mlir_value: Self._mlir_type

    def __init__(out self, *, _mlir_value: Self._mlir_type):
        self._mlir_value = _mlir_value

    def __init__(out self, *, _mlir_value: Self._mlir_lit_ref):
        self = Self(_mlir_value=__mlir_op.`lit.ref.to_pointer`(_mlir_value))

    def __init__(
        out self,
        *,
        ref[Self.origin, Self.address_space] to: Self.T,
    ):
        self = Self(
            _mlir_value=__mlir_op.`lit.ref.to_pointer`(
                __get_mvalue_as_litref(to)
            )
        )

    def __getitem__(self) -> ref[Self.origin, Self.address_space] Self.T:
        return __get_litref_as_mvalue(
            __mlir_op.`lit.ref.from_pointer`[
                _type=Self._mlir_lit_ref, name="(pointer pointee)".value
            ](self._mlir_value)
        )


def in_space[
    a: AddressSpace, o: Origin
](p: Pointer[Int, o, address_space=a]) -> Int:
    return Int(a)


def pointer_space[a: AddressSpace](ref[_, a] x: Int) -> Int:
    return in_space[a](Pointer(to=x))


def lit_ref_space[a: AddressSpace](ref[_, a] x: Int) -> Int:
    return in_space(
        Pointer[Int, origin_of(x), address_space=a](
            _mlir_value=__mlir_op.`lit.ref.to_pointer`(
                __get_mvalue_as_litref(x)
            )
        )
    )


def main():
    var x = 5
    print(pointer_space(x))
    print(lit_ref_space(x))
    var k = Pointer(to=x)._mlir_value
    var p = P[Int, origin_of(x)](_mlir_value=k)
    print(p[])
    print(P[Int, origin_of(x)](to=x)[])
    print(P[Int, origin_of(x)](_mlir_value=__get_mvalue_as_litref(x))[])
    print(p.address_space)
    print(p.mut)
    print(Int(mlir_value=__mlir_op.`pop.pointer_to_index`(p._mlir_value)) != 0)
