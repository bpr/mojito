# Every kind of compile-time parameter a `nodebug` callee declares binds at
# an inlined call, as the pin's inliner substitutes every parameter alike: a
# callable binder bound to a function, a `where` clause the checker judged
# at the call, a `comptime for` over a value pack, a struct member whose
# owner carries an origin argument (`Span.unsafe_ptr`), a `read` parameter
# passed from a place or iterated, and a copy read through a reference the
# body made (`range`'s `__iter__`). Each prints what the call computes
# either way.


@always_inline("nodebug")
def apply[f: def(Int) thin -> Int](x: Int) -> Int:
    return f(x)


def double(x: Int) -> Int:
    return x * 2


@always_inline("nodebug")
def check[n: Int]() -> Int where (n > 0, "width must be positive"):
    return n


@always_inline("nodebug")
def weighted[*values: Int]() -> Int:
    var total = 0
    comptime for i in range(len(values)):
        total += values[i] * (i + 1)
    return total


@always_inline("nodebug")
def total(xs: List[Int]) -> Int:
    var t = 0
    for x in xs:
        t += x
    return t


@always_inline("nodebug")
def bump(mut xs: List[Int]):
    for i in range(len(xs)):
        xs[i] += 1


@always_inline("nodebug")
def first_byte(s: String) -> Int:
    return Int(s.unsafe_ptr()[unsafe_offset=0])


def main():
    print(apply[double](3))
    print(check[2]())
    print(weighted[1, 2, 3](), weighted[1, 2]() + weighted[3]())
    var xs = List[Int]()
    xs.append(1)
    xs.append(2)
    bump(xs)
    print(total(xs))
    var s = Span(xs)
    print(s.unsafe_ptr()[unsafe_offset=1], len(s))
    var text = String("ab")
    print(first_byte(text))
    var acc = 0
    for i in range(3):
        acc += i
    print(acc)
