# A capture-list closure's storage struct declares the `__call__` its call
# resolves, with the closure's own signature: `f.__call__(x)` runs the
# closure, filling omitted defaults, binding keywords, solving a generic
# closure's binders, writing a `mut` capture, and raising its error, and the
# captures stay live through the call.
def main() raises:
    var k = 3

    def f(x: Int) {k} -> Int:
        return x * k

    print(f.__call__(2))
    var g = f
    print(g.__call__(5))

    def defaults(x: Int, y: Int = 5) {k} -> Int:
        return x * k + y

    print(defaults.__call__(1))
    print(defaults.__call__(1, y=2))
    print(defaults.__call__(x=2, y=1))

    def show[T: Writable](x: T) {k}:
        print(x, k)

    show.__call__(7)
    show.__call__[String]("a")

    def checked(x: Int) raises {k} -> Int:
        if x < 0:
            raise Error("negative")
        return x + k

    print(checked.__call__(1))
    try:
        _ = checked.__call__(-1)
    except e:
        print(e)

    def bump() {mut k}:
        k += 1

    bump.__call__()
    bump.__call__()
    print(k)

    var s = String("hi")

    def owned() {var s} -> String:
        return s + "!"

    print(owned.__call__())
