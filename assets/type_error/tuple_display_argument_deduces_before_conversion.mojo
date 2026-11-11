# expect: type mismatch for argument 't' to 'g': expected Tuple[Int8, Float32], found Tuple[Int, Float64]
# An argument's tuple display deduces `Tuple[Int, Float64]` before binding,
# so it does not convert element-wise as a `var` annotation's display does.
def g(t: Tuple[Int8, Float32]):
    print(t[0], t[1])


def main():
    g((1, 2.5))
