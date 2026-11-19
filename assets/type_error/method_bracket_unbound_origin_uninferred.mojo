# expect: invalid call to 'f'
# A `_` in a method call's bracket leaves an origin binder to the call's
# arguments, and no argument's place binds it here. Both compilers reject
# (pinned Mojo: "invalid call to 'f': failed to infer parameter
# 'origin_of(_mlir_origin)'").
struct S:
    def __init__(out self):
        pass

    def f[o: ImmOrigin, n: Int](self, x: Int) -> Int:
        return x + n


def main():
    var s = S()
    print(s.f[_, 3](1))
