# expect: requires an integral dtype
# `SIMD` declares no `Floatable` overload into `Float32`, so an `Intable`
# argument binds `__init__[T: Intable]`, whose `comptime assert` rejects a
# floating-point dtype, as at the pin.
struct I(Intable):
    def __init__(out self):
        pass

    def __int__(self) -> Int:
        return 3


def main():
    print(Float32(I()))
