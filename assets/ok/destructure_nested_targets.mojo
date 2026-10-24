# Recursive destructuring targets: nested tuple targets mixing `var`, `ref`
# (an immutable reference, as on the pin), and `_` leaves, a `var` prefix
# that declares every leaf, a trailing element beside a nested display, and
# nested assignment into names in scope.
def main():
    var pr = (1, (2, 3))
    (var q, (ref r, _)) = pr
    print(q, r)
    print(pr[1][0])
    var (x, (y, z)) = (4, (5, 6))
    print(x, y, z)
    var (m, n), o = (7, 8), 9
    print(m, n, o)
    var t = (1, "s")
    var (k, s) = t
    print(k, s)
    var a = 0
    var b = 0
    var c = 0
    _, (b, _) = (0, (9, 0))
    print(b)
    (a, (b, c)) = (1, (2, 3))
    print(a, b, c)
    var nested = (1, (2, (3, 4)))
    var (d, (e, (f, g))) = nested
    print(d, e, f, g)
