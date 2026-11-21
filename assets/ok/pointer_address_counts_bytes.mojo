# A pointer's integer address counts bytes, as upstream's
# `pop.pointer_to_index` reads one: an element offset over a heap buffer
# moves `Int(p)` by the pointee's size, through `Pointer.__int__` and
# through `pop.offset` and `pop.pointer_to_index` themselves, and a place
# pointer reads one address both ways. Upstream prints `8`, `8`, `3`, `16`,
# `0`, and `True`.
from std.memory.alloc import unsafe_alloc


struct Pair(Copyable, Movable):
    var a: Int
    var b: Int8

    def __init__(out self, a: Int, b: Int8):
        self.a = a
        self.b = b


def address[T: AnyType](p: Pointer[T, _]) -> Int:
    return Int(mlir_value=__mlir_op.`pop.pointer_to_index`(p._mlir_value))


def main():
    var buf = unsafe_alloc[Int](3)
    var next = buf.unsafe_offset(1)
    print(Int(next) - Int(buf))
    var second = Pointer[Int, MutUntrackedOrigin](
        _mlir_value=__mlir_op.`pop.offset`(buf._mlir_value, Int(1)._mlir_value)
    )
    print(address(second) - address(buf))
    var bytes = unsafe_alloc[Int8](4)
    var last = bytes.unsafe_offset(3)
    print(Int(last) - Int(bytes))
    var pairs = unsafe_alloc[Pair](2)
    var other = pairs.unsafe_offset(1)
    print(Int(other) - Int(pairs))
    var x = 5
    var p = Pointer(to=x)
    print(Int(p) - address(p))
    var f = SIMD[DType.float32, 1](2.5)
    var pf = Pointer(to=f)
    print(Int(pf) == address(pf))
    buf.unsafe_free()
    bytes.unsafe_free()
    pairs.unsafe_free()
