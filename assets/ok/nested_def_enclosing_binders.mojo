# A nested `def` or lambda names its enclosing function's compile-time
# parameters, in its signature, its captures' types, and its `comptime if`
# and `comptime for`, without capturing them. A generic nested `def` binds
# its own parameters at each call, spelled or not, beside the enclosing ones.


def show_each[T: Writable & Copyable](x: T):
    def inner(y: T):
        print(y)

    inner(x)


def splat[n: Int]():
    def inner() -> SIMD[DType.int32, n * 2]:
        return SIMD[DType.int32, n * 2](1)

    print(inner())


def show_copy[T: Writable & Copyable & Deinitable](x: T):
    var y = x.copy()

    def inner() {y}:
        print(y)

    inner()


def deep[n: Int]() -> Int:
    def mid() -> Int:
        def leaf() -> Int:
            return n * 3

        return leaf() + 1

    return mid()


def pick[n: Int]() -> Int:
    def inner() -> Int:
        comptime if n > 1:
            return 10
        else:
            return 20

    return inner()


def total[n: Int]() -> Int:
    def inner() -> Int:
        var s = 0
        comptime for i in range(n):
            s += i
        return s

    return inner()


def print_with[T: Writable & Copyable](x: T):
    var f = lambda (y: T): print(y)
    f(x)


def add_later[n: Int](x: Int) -> Int:
    var f = lambda (y: Int) -> Int: y + n
    return f(x)


def spelled() -> Int:
    def inner[U: AnyType](y: Int) -> Int:
        return y

    return inner[Int](3)


def compare[n: Int]() -> Int:
    def inner[k: Int]() -> Int:
        comptime if k > n:
            return 1
        else:
            return 0

    return inner[3]() * 10 + inner[1]()


def counted[n: Int]() -> Int:
    def count[*Ts: AnyType]() -> Int:
        return Ts.length + n

    return count[Int, Bool]()


def main():
    show_each(1)
    show_each("a")
    splat[2]()
    show_copy(1)
    show_copy("a")
    print(deep[2]())
    print(pick[2](), pick[0](), total[4]())
    print_with(5)
    print_with("q")
    print(add_later[3](4), spelled(), compare[2](), counted[5]())
