# Upstream's `__mlir_op` pointer ops, which its `struct Pointer` bodies are
# written in: `lit.ref.to_pointer` and `lit.ref.from_pointer` between a
# `!lit.ref` and a `!kgen.pointer`, `pop.pointer.bitcast`, `pop.offset`,
# `pop.pointer_to_index`, and the `pop.load` and `pop.store` of a
# register-passable element, over a place, a heap buffer, and a generic
# element. Upstream prints `4`, `9`, `9`, `4`, `5`, `True`, `True`, `11`, `12`,
# `30`, `2.5`, and `7`.
from std.memory.alloc import unsafe_alloc


def load[T: TrivialRegisterPassable](p: Pointer[T, _]) -> T:
    return __mlir_op.`pop.load`[
        alignment=Int(1).__mlir_index__(),
        isVolatile=False.__mlir_i1__(),
        isInvariant=False.__mlir_i1__(),
        isNonTemporal=False.__mlir_i1__(),
    ](p._mlir_value)


def store[T: TrivialRegisterPassable](p: MutPointer[T, _], value: T):
    __mlir_op.`pop.store`[
        isVolatile=False.__mlir_i1__(),
        isNonTemporal=False.__mlir_i1__(),
    ](value, p._mlir_value)


def address[T: AnyType](p: Pointer[T, _]) -> Int:
    return Int(mlir_value=__mlir_op.`pop.pointer_to_index`(p._mlir_value))


def main():
    var x = 4
    var k = __mlir_op.`lit.ref.to_pointer`(__get_mvalue_as_litref(x))
    var p = Pointer[Int, origin_of(x)](_mlir_value=k)
    print(p[])
    ref r = __get_litref_as_mvalue(
        __mlir_op.`lit.ref.from_pointer`[
            _type=Pointer[Int, origin_of(x)]._mlir_lit_ref,
            name="(pointer pointee)".value,
        ](p._mlir_value)
    )
    r = 9
    print(x)
    var imm = Pointer[Int, ImmOrigin(origin_of(x))](
        _mlir_value=__mlir_op.`pop.pointer.bitcast`[
            _type=Pointer[Int, ImmOrigin(origin_of(x))]._mlir_type
        ](p._mlir_value)
    )
    print(imm[])

    var buf = unsafe_alloc[Int](3)
    buf[unsafe_offset=0] = 4
    buf[unsafe_offset=1] = 5
    buf[unsafe_offset=2] = 6
    var second = Pointer[Int, MutUntrackedOrigin](
        _mlir_value=__mlir_op.`pop.offset`(buf._mlir_value, Int(1)._mlir_value)
    )
    print(buf[])
    print(second[])
    print(address(second) > address(buf))
    print(address(Pointer(to=x)) == address(p))

    store(second, 11)
    print(buf[unsafe_offset=1])
    var y = 5
    var py = Pointer(to=y)
    store(py, 12)
    print(load(py))
    var f = SIMD[DType.float32, 1](2.5)
    var pf = Pointer(to=f)
    store(buf, load(buf.unsafe_offset(1)) + 19)
    print(buf[unsafe_offset=0])
    print(load(pf))
    var q = Pointer(to=py)
    var loaded = load(q)
    loaded[] = 7
    print(y)
    buf.unsafe_free()
