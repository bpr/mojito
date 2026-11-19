# expect: expected Int, found LITImmOrigin
# A struct origin binder's raw origin has upstream's `LITImmOrigin` type,
# which does not convert to a declared `Int`.


struct S[o: ImmOrigin]:
    comptime v: Int = Self.o._mlir_origin

    def __init__(out self):
        pass


def main():
    var x = 1
    print(S[origin_of(x)].v)
