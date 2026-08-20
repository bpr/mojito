from std.memory import unsafe_alloc


def total(p: Pointer[Int, MutUntrackedOrigin], n: Int) -> Int:
    var acc = 0
    var i = 0
    while i < n:
        acc = acc + p[i]
        i = i + 1
    return acc


def main():
    var p = unsafe_alloc[Int](5)
    var i = 0
    while i < 5:
        p[i] = i * 10
        i = i + 1
    print(p[0], p[4], total(p, 5))
    p.unsafe_free()
    var q = unsafe_alloc[Float64](2, alignment=32)
    q[0] = 1.5
    q[1] = q[0] * 2.0
    print(q[1])
    q.unsafe_free()
