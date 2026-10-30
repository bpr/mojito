# expect: type mismatch for argument to 'add'
# A bare `Scalar` argument is parameterized over its own dtype, which the
# argument solves; a later argument typed `type_of(x)` takes that dtype only,
# so a `Float64` passed beside an `Int32` is rejected, as at the pin.
def add(x: Scalar, y: type_of(x)) -> type_of(x):
    return x + y


def main():
    print(add(Int32(1), Float64(2.5)))
