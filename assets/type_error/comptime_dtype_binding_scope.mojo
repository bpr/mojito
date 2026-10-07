# expect: 'lane' does not name a DType in scope
# A local `comptime` binding is visible to the rest of its own block only: a
# `comptime lane` in `first` is not in scope in `second`.
def first():
    comptime lane = DType.int8
    print(Scalar[lane](1))


def second(a: Int) -> Int:
    var x: Scalar[lane] = 1
    return a + Int(x)


def main():
    first()
    print(second(2))
