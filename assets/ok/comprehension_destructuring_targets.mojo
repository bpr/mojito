# A comprehension's `for` clause destructures each element as a `for`
# statement does: tuple and nested targets, `var`/`ref` leaves, filters
# over the leaves, and a later clause iterating a leaf.
def main() raises:
    var pairs: List[Tuple[Int, String]] = [(1, "a"), (2, "b")]
    var xs = [a * 2 for a, _ in pairs]
    print(len(xs), xs[0], xs[1])
    var nested: List[Tuple[Int, Tuple[Int, Int]]] = [(1, (2, 3)), (4, (5, 6))]
    var ys = [x + y for _, (x, y) in nested if x > 1]
    print(len(ys), ys[0], ys[1])
    var d = {k: v for k, v in pairs}
    print(len(d), d[2])
    var s = {a for a, _ in pairs}
    print(len(s))
    var vs = [x for (var x, _) in pairs]
    var rs = [x for ref (x, _) in pairs]
    print(vs[1], rs[1])
    var first: List[Int] = [2, 3]
    var second: List[Int] = [20]
    var grouped = List[Tuple[Int, List[Int]]]()
    grouped.append((1, first^))
    grouped.append((10, second^))
    var flat = [a + b for a, row in grouped for b in row]
    print(len(flat), flat[0], flat[1], flat[2])
