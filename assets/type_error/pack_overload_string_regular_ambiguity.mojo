# expect: ambiguous overloaded call
# Two type-pack overloads differ by one regular `String` parameter before the
# collector. A `String` binds by reference whether the parameter or the pack
# takes it, so neither candidate binds more arguments by value and the call is
# ambiguous, as the pinned Mojo reports. With an `Int` there instead, the
# second overload binds one more argument by value and is selected
# (`assets/ok/pack_overload_arity.mojo`).
def g[*Ts: Writable](a: Int, *rest: *Ts) -> Int:
    return 2


def g[*Ts: Writable](a: Int, b: String, *rest: *Ts) -> Int:
    return 3


def main():
    var x = 1
    var s = String("s")
    print(g(x, s, x))
