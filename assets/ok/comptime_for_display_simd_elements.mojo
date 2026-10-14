# A `comptime for` over a list display or an applied list iterates the
# collection's own `__iter__()`, so a SIMD element is a loop variable too.
def mk() -> List[String]:
    return ["u", "v"]


def main():
    comptime for s in mk():
        print(s)
    comptime for v in [SIMD[DType.int32, 2](1, 2), SIMD[DType.int32, 2](3, 4)]:
        print(v)
