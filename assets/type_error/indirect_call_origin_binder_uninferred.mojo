# expect: failed to infer parameter 'origin_of(_mlir_origin)'
# A call through a callable value whose contract declares an origin
# binder no argument names fails to infer it, as a direct call does.
def f[o: ImmOrigin, n: Int]() -> Int:
    return n


def run(g: def[o: ImmOrigin]() thin -> Int) -> Int:
    return g()


def main():
    print(run(f[_, 7]))
