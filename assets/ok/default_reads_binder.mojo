# Omitted arguments whose default reads a compile-time parameter in scope:
# the struct's through `Self`, the method's or function's own, an enclosing
# function's from a nested `def`, and a type parameter. Each instance runs
# the default with its own arguments.
struct V[n: Int]:
    def __init__(out self):
        pass

    def m(self, x: Int = Self.n * 2) -> Int:
        return x

    def k[j: Int](self, x: Int = Self.n + j) -> Int:
        return x

    @staticmethod
    def s(x: Int = Self.n - 1) -> Int:
        return x


def outer[n: Int]() -> Int:
    def inner(x: Int = n + 1) -> Int:
        return x

    return inner()


def g[n: Int](x: Int = n, y: Int = n * 10) -> Int:
    return x + y


def t[T: Defaultable & Writable](x: T = T()) -> String:
    return String(x)


def main():
    print(V[3]().m(), V[4]().m(), V[3]().m(1))
    print(V[3]().k[10](), V[5]().k[1]())
    print(V[2].s(), V[9].s())
    print(outer[4](), outer[5]())
    print(g[7](), g[2](y=1), g[3](1))
    print(t[Int](), t[String]() == "")
