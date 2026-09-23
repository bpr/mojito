# expect: expected VariadicList[Int], found a variadic pack
# A heterogeneous pack forwards into a collector that is itself a pack
# (`*b: *Us`), never into a homogeneous `*b: Int`.
def sink(*b: Int) -> Int:
    return 1


def relay[*Ts: Copyable](*a: *Ts) -> Int:
    return sink(*a)


def main():
    print(relay(1, 2))
