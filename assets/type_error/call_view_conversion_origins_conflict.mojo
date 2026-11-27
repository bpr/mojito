# expect: arguments bind conflicting origins for parameter 'b'
# Each argument's implicit view conversion solves the one origin binder
# both parameters name, from a different list.
def both[o: ImmOrigin](a: Span[Byte, o], b: Span[Byte, o]) -> Int:
    return len(a) + len(b)


def main():
    var xs: List[Byte] = [1, 2, 3]
    var ys: List[Byte] = [1]
    print(both(xs, ys))
