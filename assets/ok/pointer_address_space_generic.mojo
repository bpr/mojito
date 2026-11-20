# A `Pointer`'s keyword-only `address_space` parameter, upstream's
# `Pointer[T, origin, *, address_space: AddressSpace = .GENERIC]`, in the
# generic space every host pointer lives in: spelled in a parameter type,
# deduced by a binder, cast to its own space by
# `unsafe_address_space_cast`, and carried by the raw `!kgen.pointer<T, as>`
# and `!lit.ref<T, origin, as>` a `Pointer` wraps and is built from.
# Upstream prints `5`, `0`, `5`, `5`, `5`, `5`, and `6`.
from std.memory import AddressSpace


struct Raw[T: AnyType, a: AddressSpace]:
    comptime Address = __mlir_type[
        `!kgen.pointer<`, Self.T, `, `, Self.a._value._mlir_value, `>`
    ]
    comptime Reference = __mlir_type[
        `!lit.ref<`,
        Self.T,
        `, `,
        MutAnyOrigin._mlir_origin,
        `, `,
        Self.a._value._mlir_value,
        `>`,
    ]


def read(p: Pointer[Int, _, address_space=AddressSpace.GENERIC]) -> Int:
    return p[]


def space_of[a: AddressSpace](p: Pointer[Int, _, address_space=a]) -> Int:
    return Int(a)


def round_trip[o: Origin](p: Pointer[Int, o]) -> Int:
    var address: Raw[Int, AddressSpace.GENERIC].Address = p._mlir_value
    return Pointer[Int, o](_mlir_value=address)[]


def main():
    var x = 5
    print(read(Pointer(to=x)))
    print(space_of(Pointer(to=x)))
    print(Pointer(to=x).unsafe_address_space_cast[AddressSpace.GENERIC]()[])
    print(Pointer(to=x).unsafe_address_space_cast[.GENERIC]()[])
    print(read(Pointer(to=x).unsafe_address_space_cast()))
    print(round_trip(Pointer(to=x)))
    comptime Literal = __mlir_type[`!kgen.pointer<`, Int, `, `, `0`, `>`]
    var y = 6
    var q: Literal = Pointer(to=y)._mlir_value
    print(Pointer[Int, origin_of(y)](_mlir_value=q)[])
