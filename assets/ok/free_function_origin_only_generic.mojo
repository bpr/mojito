# A function whose only parameters are origins is generic over them: a
# call binds its origin from the bracket or infers it from the argument,
# and a bare reference is a runtime closure that infers it per call.
def first[o: ImmOrigin](p: Pointer[Int, o]) -> Int:
    return p[] + 1


def pick[o: ImmOrigin](ref [o] x: Int) -> ref [o] Int:
    return x


def main():
    var x = 3
    var y = 7
    print(first[origin_of(x)](Pointer(to=x)))
    print(first[o=origin_of(y)](Pointer(to=y)))
    print(first(Pointer(to=x)))
    print(pick[origin_of(y)](y))
    var g = first
    print(g(Pointer(to=x)))
    print(g(Pointer(to=y)))
