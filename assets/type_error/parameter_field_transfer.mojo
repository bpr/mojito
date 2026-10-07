# expect: cannot transfer from a parameter expression
# A field of a compile-time parameter is a parameter expression, not
# storage: `q.s^` has nothing to move out of, as at the pin.
@fieldwise_init
struct Q(ImplicitlyCopyable):
    var a: Int
    var s: String


def f[q: Q]():
    var t = q.s^
    print(t)


def main():
    f[Q(8, "x")]()
