# expect: expected LITOrigin[mut], found MutOrigin
# An `Origin` value is not the raw `!lit.origin` attribute `Origin`'s
# `_mlir_origin` holds, as at the pin.


def main():
    var x = 1
    comptime O = Origin[_mlir_origin=origin_of(x)]
    print(O.mut)
    x += 1
