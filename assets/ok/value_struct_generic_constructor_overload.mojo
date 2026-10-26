# A generic constructor declared beside another one on a struct with a value
# parameter: the template serves both, so `P[Int, 3](7)` and `P[Int, 3](s)`
# select the generic constructor and the rvalue selects the `var` one.
struct P[U: AnyType, n: Int]:
    var v: Int

    def __init__(out self, var a: String):
        self.v = 1

    def __init__[T: Writable](out self, a: T):
        self.v = 2 + Self.n


def main():
    var s = String("s")
    print(P[Int, 3](7).v)
    print(P[Int, 3](s).v)
    print(P[Int, 3](String("t")).v)
