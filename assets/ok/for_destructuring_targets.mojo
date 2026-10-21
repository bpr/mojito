# A `for` target destructures each element as an assignment target does,
# with a bare name binding immutably, as on the pin: tuple, nested, and
# sequence targets, explicit `var`/`ref` leaves, places, and shadowing. A
# bare name binds its piece without copying it, so a piece that does not
# copy implicitly still binds.
@fieldwise_init
struct P(Copyable):
    var x: Int
    var y: Int


struct C(Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def main() raises:
    var pairs: List[Tuple[Int, String]] = [(1, "a"), (2, "b")]
    for a, b in pairs:
        print(a, b)
    for (var a, _) in pairs:
        a += 10
        print(a)
    for ref (a, b) in pairs:
        print(a, b)
    for var (a, b) in pairs:
        a += 1
        b += "!"
        print(a, b)
    print(pairs[0][0], pairs[0][1])
    var nested: List[Tuple[Int, Tuple[Int, Int]]] = [(1, (2, 3)), (4, (5, 6))]
    for i, (var x, ref y) in nested:
        x += 100
        print(i, x, y)
    for (i, (x, y)) in nested:
        print(i + x + y)
    for (a,) in [(7,), (8,)]:
        print(a)
    var x = 0
    for x, y in [(1, 2)]:
        print(x, y)
    print(x)
    var p = P(0, 0)
    for p.x, p.y in [(3, 4), (5, 6)]:
        print(p.x, p.y)
    var q: List[Int] = [0]
    for q[0] in [7, 8]:
        print(q[0])
    var rows: List[List[Int]] = [[1, 2, 3], [4, 5]]
    for [first, *rest] in rows:
        print(first, len(rest))
    for [f, var s, *_] in rows:
        s += 1
        print(f, s, rows[0][1])
    var trows: List[List[Tuple[Int, Int]]] = [[(1, 2), (3, 4)]]
    for [(a, b), *_] in trows:
        print(a, b)
    try:
        var ragged: List[List[Int]] = [[1, 2, 3], [4]]
        for [g, h, *_] in ragged:
            print(g, h)
    except e:
        print(e)
    var cs = List[Tuple[C, Int]]()
    cs.append((C(1), 2))
    for c, n in cs:
        print(c.v, n)
    for _ in pairs:
        print("tick")
