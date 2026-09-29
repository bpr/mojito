# A free function's own origin binder, bound from its arguments, is also
# bound in its result: `f(Span(xs))` has type `Span[Int, origin_of(xs)]`, at
# the top of an argument or nested in one (`Holder[Span[Int, o]]`). An
# origin a generic function receives only through a type argument
# (`pick`'s `T` bound to `Span[Int, origin_of(xs)]`) is immutable here, the
# list being a read parameter, so passing two values carrying it is no
# aliasing.
@fieldwise_init
struct Holder[T: Copyable & Deinitable](Copyable):
    var item: Self.T


def f[o: Origin](s: Span[Int, o]) -> Span[Int, o]:
    return s


def g[o: Origin](h: Holder[Span[Int, o]]) -> Holder[Span[Int, o]]:
    return h.copy()


def pick[T: Copyable](items: List[T], default: T) -> T:
    if len(items) > 0:
        return items[0].copy()
    return default.copy()


def run(xs: List[Int]):
    var a = f(Span(xs))
    var b: Span[Int, origin_of(xs)] = a
    var h = g(Holder[Span[Int, origin_of(xs)]](Span(xs)))
    h.item = Span(xs)
    var l = List[Span[Int, origin_of(xs)]]()
    l.append(Span(xs))
    print(b[1], len(h.item), pick(l, Span(xs))[2])
    print(xs[0])


def main():
    run([1, 2, 3])
