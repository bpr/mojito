# A plain `@always_inline` callee is inlined before elaboration when its
# body is small enough, as at the pin, and keeps its calls past the size
# bound: 31 `s += x` statements inline and 32 do not, ten `if`s inline and
# eleven do not, four `while` loops inline and five do not, one `print`
# inlines and two do not, and a `for` loop never does. Each prints what the
# call computes either way.


@no_inline
def bump(x: Int) -> Int:
    return x + 1


@always_inline
def add_three(x: Int) -> Int:
    return x + 3


@always_inline
def clamp(x: Int, low: Int, high: Int) -> Int:
    if x < low:
        return low
    if x > high:
        return high
    return x


@always_inline
def count_up(x: Int) -> Int:
    var s = x
    while s < 100:
        s += x
    return s


@always_inline
def sum_to(x: Int) -> Int:
    var s = 0
    for i in range(x):
        s += i
    return s


@always_inline
def ten_steps(x: Int) -> Int:
    var s = x
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    return s


@always_inline
def eleven_steps(x: Int) -> Int:
    var s = x
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    s = s * x + x - 3
    return s


@always_inline
def bumped_fifteen(x: Int) -> Int:
    var s = x
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    return s


@always_inline
def bumped_sixteen(x: Int) -> Int:
    var s = x
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    s += bump(x)
    return s


@always_inline
def show(x: Int):
    print("show", x)


@always_inline
def show_twice(x: Int):
    print("first", x)
    print("second", x)


struct Pair:
    var a: Int
    var b: Int

    def __init__(out self, a: Int, b: Int):
        self.a = a
        self.b = b

    @always_inline
    def total(self) -> Int:
        return self.a + self.b

    @always_inline
    def scale(mut self, k: Int):
        self.a *= k
        self.b *= k


def main():
    print(add_three(4))
    print(clamp(5, 1, 3), clamp(-2, 1, 3), clamp(2, 1, 3))
    print(count_up(7))
    print(sum_to(5))
    print(ten_steps(2), eleven_steps(2))
    print(bumped_fifteen(1), bumped_sixteen(1))
    show(9)
    show_twice(10)
    var p = Pair(2, 5)
    p.scale(3)
    print(p.total())
