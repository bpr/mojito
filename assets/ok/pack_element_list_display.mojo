# A pack element read under `comptime for` fills a list display, directly
# and through a `var` each unrolled iteration declares afresh, for `Int`,
# `String`, and `Bool` elements alike.
def first[T: Copyable & Writable](xs: List[T], d: T) -> T:
    return xs[0].copy()


def walk[*Ts: ImplicitlyCopyable & Writable & Deinitable](*args: *Ts):
    comptime for i in range(Ts.length):
        print(first([args[i], args[i]], args[i]))
        var v = args[i]
        print(first([v, v], v))


def main():
    walk(1, "s", True)
