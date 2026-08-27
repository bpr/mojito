# List method growth: capacity/length-fill constructors, reserve/resize/
# shrink/swap_elements/capacity, unsafe_get/unsafe_set, consuming extend,
# concatenation and repetition operators, equality, __bool__, raising
# index and try_index — identical output on both compilers.
def main() raises:
    var a: List[Int] = List[Int](capacity=10)
    print(len(a), a.capacity(), Bool(a))
    var b: List[Int] = List[Int](length=3, fill=7)
    print(len(b), b[0], b[2], Bool(b))
    b.reserve(20)
    print(b.capacity(), len(b))
    b.resize(5, 9)
    print(len(b), b[3], b[4])
    b.resize(2, 0)
    print(len(b), b[1])
    b.shrink(1)
    print(len(b))
    var s: List[Int] = [10, 20, 30]
    s.swap_elements(0, 2)
    print(s[0], s[2])
    print(s.unsafe_get(1))
    s.unsafe_set(1, 99)
    print(s[1])
    var x: List[Int] = [1, 2]
    var y: List[Int] = [3, 4]
    x.extend(y^)
    print(len(x), x[3])
    var p: List[Int] = [1, 2]
    var q: List[Int] = [1, 2]
    var r: List[Int] = [2, 1]
    print(Bool(p == q), Bool(p != q), Bool(p == r))
    var cat = p + q.copy()
    print(len(cat), cat[2])
    p += r^
    print(len(p), p[2])
    var rep = q * 3
    print(len(rep), rep[4])
    q *= 2
    print(len(q), q[3])
    q *= 0
    print(len(q))
    var spanned: List[Int] = [1, 2, 3]
    var source: List[Int] = [4, 5, 6]
    spanned.extend(Span(source))
    print(len(spanned), spanned[5], len(source))
    var f: List[Int] = [5, 6, 5]
    print(f.index(6))
    var t = f.try_index(5)
    print(Bool(t), t.or_else(-1))
    print(Bool(f.try_index(42)))
    try:
        var i = f.index(42)
        print(i)
    except e:
        print(e)
