# expect: requires an integral dtype
# A lone `Bool` into a one-lane instance whose dtype is a parameter binds
# `__init__[T: Intable]`, so the `bool` instance fails its integral-dtype
# assert, as at the pin.
def lane[dt: DType](b: Bool) -> SIMD[dt, 1]:
    return SIMD[dt, 1](b)


def main():
    print(lane[DType.bool](True))
