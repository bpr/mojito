# A byte written through a `Pointer.unsafe_bitcast[Byte]` view reads back
# as written, as the pin's memory keeps every byte: a struct's padding, a
# `Bool` byte's high bits until a typed store of the `Bool` rewrites them,
# a `Float32` or `Float16` signaling NaN's exact bits, also across a typed
# copy, and a heap `bool` vector's bytes past its packed lanes across a
# typed store of the vector, beside its packed byte's unused high bits.
from std.memory.alloc import unsafe_alloc


@fieldwise_init
struct Padded(Copyable, Movable):
    var a: Int8
    var b: Int


@fieldwise_init
struct Flagged(Copyable, Movable):
    var f: Bool
    var n: Int8


def padding():
    var p = Padded(1, 2)
    var bytes = Pointer(to=p).unsafe_bitcast[Byte]()
    bytes.unsafe_offset(1)[] = 171
    print(bytes.unsafe_offset(1)[], p.a, p.b)
    var heap: Pointer[Padded, MutUntrackedOrigin] = unsafe_alloc[Padded](1)
    heap.unsafe_write(Padded(3, 4))
    var heap_bytes = heap.unsafe_bitcast[Byte]()
    heap_bytes.unsafe_offset(7)[] = 205
    print(heap_bytes.unsafe_offset(7)[], heap[].a, heap[].b)


def bool_bytes():
    var q = Flagged(False, 3)
    var bytes = Pointer(to=q).unsafe_bitcast[Byte]()
    bytes[] = 2
    q.n = 5
    print(bytes[], q.n)
    q.f = False
    print(bytes[])
    var b: Pointer[Bool, MutUntrackedOrigin] = unsafe_alloc[Bool](1)
    b[] = False
    b.unsafe_bitcast[Byte]()[] = 2
    print(b.unsafe_bitcast[Byte]()[])
    b[] = False
    print(b.unsafe_bitcast[Byte]()[])


def nan_bits():
    var f: Float32 = 0.0
    var bytes = Pointer(to=f).unsafe_bitcast[Byte]()
    bytes[] = 1
    bytes.unsafe_offset(1)[] = 0
    bytes.unsafe_offset(2)[] = 0x80
    bytes.unsafe_offset(3)[] = 0x7F
    print(Pointer(to=f).unsafe_bitcast[UInt32]()[])
    var h: Float16 = 0.0
    var half = Pointer(to=h).unsafe_bitcast[Byte]()
    half[] = 1
    half.unsafe_offset(1)[] = 0x7C
    print(Pointer(to=h).unsafe_bitcast[UInt16]()[])
    var source: Pointer[Float32, MutUntrackedOrigin] = unsafe_alloc[Float32](1)
    source[] = 0.0
    source.unsafe_bitcast[UInt32]()[] = 2139095041
    var copy: Pointer[Float32, MutUntrackedOrigin] = unsafe_alloc[Float32](1)
    copy[] = source[]
    print(copy.unsafe_bitcast[UInt32]()[])


def mask_bytes():
    var m: Pointer[SIMD[DType.bool, 4], MutUntrackedOrigin] = unsafe_alloc[
        SIMD[DType.bool, 4]
    ](1)
    m[] = SIMD[DType.bool, 4](True, False, True, True)
    var bytes = m.unsafe_bitcast[Byte]()
    bytes.unsafe_offset(1)[] = 112
    print(bytes[], bytes.unsafe_offset(1)[])
    m[] = SIMD[DType.bool, 4](False, False, True, True)
    print(bytes[], bytes.unsafe_offset(1)[])
    bytes[] = 0xFD
    print(bytes[], m[])


def main():
    padding()
    bool_bytes()
    nan_bits()
    mask_bytes()
