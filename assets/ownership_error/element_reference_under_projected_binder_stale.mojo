# expect: use of invalidated interior reference
# An element reference returned under an origin parameter's element interior
# names the caller's list's element interior, so clearing the list stales it,
# as at the pin.
struct It[o: ImmOrigin]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] l: List[Int]):
        self.src = Pointer(to=l)

    def first(self) -> ref[Self.o._get_owned_interior["element"]] Int:
        return self.src[][0]


def main():
    var l: List[Int] = [1, 2]
    var it = It(l)
    ref x = it.first()
    l.clear()
    print(x)
