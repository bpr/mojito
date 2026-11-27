# expect: failed to infer parameter 'origin_of(_mlir_origin)'
# An `ImmOrigin` binder no argument names fails to infer, named as
# upstream names the raw origin its type declares.
def h[o: ImmOrigin]() -> Int:
    return 3


def main():
    print(h())
