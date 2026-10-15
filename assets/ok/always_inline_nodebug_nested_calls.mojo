# Calls to an `@always_inline("nodebug")` nested `def` are inlined into the
# body that declares it: its captures are that body's places, read and
# written in place, and an enclosing declaration's parameters read as its
# own. A copy capture keeps the call. Each prints what the call computes
# either way.


struct Scaler[n: Int]:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def scaled(self) -> Int:
        @always_inline("nodebug")
        def mul(x: Int) -> Int:
            return x * Self.n

        return mul(self.v)


def add_twice(x: Int) -> Int:
    @always_inline("nodebug")
    def add(y: Int) -> Int:
        return y + 1

    return add(add(x))


def captured(x: Int) -> Int:
    var base = x * 10

    @always_inline("nodebug")
    def plus(y: Int) {base} -> Int:
        return base + y

    return plus(3) + plus(4)


def counted() -> Int:
    var count = 0

    @always_inline("nodebug")
    def bump() {mut count}:
        count += 1

    bump()
    bump()
    return count


def pushed() -> Int:
    var acc: List[Int] = []

    @always_inline("nodebug")
    def push(v: Int) {mut acc}:
        acc.append(v)

    for i in range(4):
        push(i * i)
    return len(acc) + acc[3]


def scaled_by[k: Int](x: Int) -> Int:
    @always_inline("nodebug")
    def mul(y: Int) -> Int:
        return y * k

    return mul(x)


def deep[n: Int]() -> Int:
    def mid() -> Int:
        @always_inline("nodebug")
        def leaf() -> Int:
            return n * 3

        return leaf() + 1

    return mid()


def generic_nested(x: Int) -> Int:
    var base = x

    @always_inline("nodebug")
    def times[k: Int]() {base} -> Int:
        return base * k

    return times[2]() + times[3]()


def defaulted() -> Int:
    @always_inline("nodebug")
    def scale(x: Int, factor: Int = 2, *, offset: Int = 0) -> Int:
        return x * factor + offset

    return scale(3) + scale(3, 4) + scale(offset=1, x=3)


def checked(x: Int) -> Int:
    var tag = String("limit")

    @always_inline("nodebug")
    def check(y: Int) raises {tag} -> Int:
        if y > 2:
            raise Error(tag)
        return y + tag.byte_length()

    try:
        return check(x)
    except e:
        print("caught", e)
        return -1


def shown[T: Writable & Copyable](value: T):
    @always_inline("nodebug")
    def show(prefix: String) {value}:
        print(prefix, value)

    show("value:")


def copied() -> Int:
    var x = 5

    @always_inline("nodebug")
    def f() {var x} -> Int:
        return x + 1

    x = 7
    return f()


def main():
    print(Scaler[3](4).scaled())
    print(add_twice(4))
    print(captured(2))
    print(counted())
    print(pushed())
    print(scaled_by[3](5))
    print(deep[5]())
    print(generic_nested(4))
    print(defaulted())
    print(checked(1))
    print(checked(5))
    shown(String("hi"))
    shown(42)
    print(copied())
