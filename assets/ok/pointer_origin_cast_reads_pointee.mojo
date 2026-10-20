# A pointer whose origin was cast, or one a method returns under a wider
# origin than the place it addresses, dereferences its own address: the
# origin only keeps an owner alive, as on the pin.
@fieldwise_init
struct Pair(Writable, Copyable):
    var x: Int
    var y: Int

    def ptr(ref self) -> Pointer[Int, origin_of(self)]:
        return Pointer(to=self.y).unsafe_origin_cast[origin_of(self)]()


def main():
    var a = 1
    var b = 2
    var p = Pointer(to=b).unsafe_origin_cast[origin_of(a)]()
    print(p[])
    p[] = 7
    p[] += 1
    var p2 = p
    print(p2[], a, b)
    print(Pointer(to=a).unsafe_origin_cast[origin_of(b)]()[], a)

    var s = Pair(5, 6)
    print(s.ptr()[])
    var q = s.ptr()
    q[] = 9
    print(q[], s.x, s.y)

    var u = Pair(1, 2)
    var v = Pair(3, 4)
    var r = Pointer(to=v).unsafe_origin_cast[origin_of(u)]()
    print(r[].x)
    r[].y = 40
    print(u, v)

    var l1: List[Int] = [1, 2]
    var l2: List[Int] = [3, 4]
    var w = Pointer(to=l2).unsafe_origin_cast[origin_of(l1)]()
    w[].append(5)
    print(len(w[]))
    print(len(l1), len(l2))
