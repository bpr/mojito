# Every kind of compile-time parameter a `nodebug` callee declares binds at
# an inlined call, as the pin's inliner substitutes every parameter alike: a
# callable binder bound to a function, a `where` clause the checker judged
# at the call, a `comptime for` over a value pack, a struct member whose
# owner carries an origin argument (`Span.unsafe_ptr`), a `read` parameter
# passed from a place or iterated, a copy read through a reference the body
# made (`range`'s `__iter__`), and a `conforms_to` test at a concrete
# binding, of a type or a pack, as is a `TypeList` query (`contains`, `all`,
# `length`) at a concrete pack. Each prints what the call computes either
# way.

from std.traits import IsTriviallyCopyable


@fieldwise_init
struct Solo(Movable):
    var x: Int


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


@always_inline("nodebug")
def copy_tag[T: AnyType]() -> Int:
    comptime if conforms_to(T, Copyable):
        return 1
    else:
        return 2


@always_inline("nodebug")
def all_copy_tag[*Ts: AnyType]() -> Int:
    comptime if conforms_to(Ts.values, Copyable):
        return 1
    else:
        return 2


@always_inline("nodebug")
def has_int[*Ts: AnyType]() -> Int:
    return 1 if TypeList[Ts.values]().contains[Int]() else 2


@always_inline("nodebug")
def all_trivial[*Ts: AnyType]() -> Int:
    return 1 if TypeList[Ts.values]().all[IsTriviallyCopyable]() else 2


@always_inline("nodebug")
def pair_tag[*Ts: AnyType]() -> Int:
    comptime if TypeList[Ts.values]().length == 2:
        return 1
    else:
        return 2


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
    print(copy_tag[Int](), copy_tag[Solo]())
    print(all_copy_tag[Int, Bool](), all_copy_tag[Int, Solo]())
    print(has_int[Int, String](), has_int[String, Bool]())
    print(all_trivial[Int, Bool](), all_trivial[Int, String]())
    print(pair_tag[Int, String](), pair_tag[Int]())
