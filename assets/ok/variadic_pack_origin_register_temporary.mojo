# A register-passable temporary a `read` collector gathers lives as long as
# the reference a call returns through the pack origin, as upstream's pack
# refers to each element where it lies: an operator result, a literal, a
# construction, and a value in a loop each stay readable after the call.
@fieldwise_init
struct P(ImplicitlyCopyable, Writable):
    var a: Int
    var b: Int


def first[*Ts: Writable](*args: *Ts) -> ref[args.origin] Ts[0]:
    return args[0]


def main():
    var n = 3
    var s = String("a")
    print(first(n + 1, s))
    print(first(5, s))
    print(first(s, n + 1))
    ref r = first(n + 2, s)
    print(r)
    var x = 2.5
    print(first(x * 2.0, 7))
    var y = first(P(1, 2), s)
    print(y.a, y.b)
    for i in range(3):
        print(first(i * 10, s))
    print(first(True, 1))
