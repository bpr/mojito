# A parameter typed by a conditional over a value binder takes the arm the
# call's explicit parameters select, directly and through a generic alias.
comptime Pick[n: Int] = Int if n > 0 else Float64


def direct[n: Int](x: Int if n > 0 else Float64) -> Int:
    return n


def aliased[n: Int](x: Pick[n]) -> Int:
    return n


def main():
    print(direct[1](2))
    print(direct[0](2.5))
    print(aliased[3](4))
    print(aliased[-1](0.5))
