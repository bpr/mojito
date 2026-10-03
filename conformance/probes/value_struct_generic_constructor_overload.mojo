# A generic constructor declared beside another one on a struct with a value
# parameter. The pin prints `5` `5` `1`; Mojito stops at run time ("vm backend
# does not support the built-in or callee 'P.__init__$ov$T$Writable' yet").
# `docs/roadmap.md` 3.105. When Mojito runs it, move this to `assets/ok/`.
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
