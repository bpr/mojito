# A `UInt` struct value parameter reads at `UInt`, and an integer literal
# bound to a sized-scalar value parameter materializes into its lane,
# wrapping as `UInt8(300)` does.


struct Counter[n: UInt]:
    def __init__(out self):
        pass

    def next(self) -> UInt:
        return Self.n + 1


def lane[x: Scalar[DType.uint8]]() -> UInt8:
    return x


def main():
    print(Counter[5]().next(), Counter[18446744073709551614]().next())
    print(lane[250](), lane[300]())
