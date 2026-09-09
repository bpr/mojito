# Multi-lane vectors cross every storage and call boundary as plain values:
# struct fields (nested, inside a List), arguments, returns, tuple elements,
# reassignment, copies that diverge, and lane writes through fields.
@fieldwise_init
struct Pair(Copyable, ImplicitlyCopyable, Movable):
    var lo: SIMD[DType.int32, 4]
    var hi: SIMD[DType.float64, 2]

@fieldwise_init
struct Crate(Copyable, ImplicitlyCopyable, Movable):
    var tag: Int
    var pair: Pair

def bump(v: SIMD[DType.int32, 4], by: Int32) -> SIMD[DType.int32, 4]:
    return v + by

def total(p: Pair) -> Float64:
    return Float64(p.lo.reduce_add()) + p.hi.reduce_add()

def scale(mut p: Pair, k: Int32):
    p.lo = p.lo * k
    p.hi[1] = p.hi[1] * 2.0

def main():
    var a = SIMD[DType.int32, 4](1, 2, 3, 4)
    var b = bump(a, 10)
    print(a, b, bump(b, -1))
    var p = Pair(a, SIMD[DType.float64, 2](0.5, 1.5))
    var boxes = List[Crate]()
    boxes.append(Crate(1, p))
    boxes.append(Crate(2, Pair(b, SIMD[DType.float64, 2](2.5))))
    print(boxes[0].pair.lo, boxes[1].pair.hi, total(boxes[1].pair))
    scale(boxes[0].pair, 3)
    print(boxes[0].pair.lo, boxes[0].pair.hi, total(boxes[0].pair))
    var t = (a, SIMD[DType.uint8, 8](9), 7)
    print(t[0], t[1], t[2], t[1].reduce_add())
    var c = a
    c[2] = 30
    a = a - 1
    print(a, c)
    var second = boxes[1]
    second.pair.lo[0] = 100
    second.pair.hi = second.pair.hi + 1.0
    boxes[1] = second
    print(boxes[1].pair.lo, boxes[1].pair.hi, len(boxes), second.tag)
