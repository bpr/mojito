# A capture-list closure with a `*args` or `**kwargs` collector declares
# the storage `__call__` its call resolves, as every closure does:
# `f.__call__(...)` and a call through a copy of the closure forward the
# collectors, bind keyword-only parameters after `*args`, and raise.
def main() raises:
    var k = 3

    def v(a: Int, *xs: Int, scale: Int = 1) {k} -> Int:
        var s = k + a
        for x in xs:
            s += x
        return s * scale

    print(v.__call__(1, 2))
    print(v.__call__(1, 2, 3, scale=2))
    var g = v
    print(g(4))
    print(g.__call__(1))

    def owned(var *xs: String) {k} -> Int:
        var n = k
        for x in xs:
            n += x.byte_length()
        return n

    print(owned.__call__("ab", "c"))

    def first(*xs: Int) raises {k} -> Int:
        if len(xs) == 0:
            raise Error("empty")
        return xs[0] + k

    print(first.__call__(4))
    try:
        _ = first.__call__()
    except e:
        print(e)

    def c(var **kw: Int) {k} -> Int:
        return k + len(kw)

    print(c.__call__(a=1))
    var d = c
    print(d(a=1, b=2))
