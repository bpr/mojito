# expect: expected ImmOrigin, found LITMutOrigin
# A raw origin `X._mlir_origin` is not the `Origin` value a struct's
# origin binder takes, as at the pin.


struct S[o: ImmOrigin]:
    def __init__(out self):
        pass


def main():
    var x = 1
    _ = S[origin_of(x)._mlir_origin]()
    x += 1
