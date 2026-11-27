# expect: failed to infer parameter 'origin_of(_mlir_origin)'
# A `_` in a call's bracket leaves an origin binder to the call's
# arguments, and no argument's place binds it here. Both compilers reject
# (pinned Mojo: "invalid call to 'f': failed to infer parameter
# 'origin_of(_mlir_origin)'").
def f[o: ImmOrigin, n: Int](x: Int) -> Int:
    return x + n


def main():
    print(f[_, 3](1))
