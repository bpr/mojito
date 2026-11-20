# A function type whose only parameters are origins is a generator over
# them, so a runtime parameter declares one, and each call through it
# infers the origin from its argument's place.


def first[o: ImmOrigin](p: Pointer[Int, o]) -> Int:
    return p[]


def apply(f: def[o: ImmOrigin](Pointer[Int, o]) thin -> Int, x: Int) -> Int:
    return f(Pointer(to=x)) + 1


def main():
    print(apply(first, 3))
    var g = first
    var y = 4
    print(g(Pointer(to=y)))
    print(apply(g, 5))
