# expect: cannot return reference with incompatible origin
# A return contract over a binder's owned interior names that interior, not
# the binder: the list a pointer over `Self.o` reaches is `o` itself, so
# returning it under `Self.o._get_owned_interior["element"]` is rejected, as
# at the pin: 'origin_of(o)' vs 'origin_of(o["element"])'.
struct It[o: ImmOrigin]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] l: List[Int]):
        self.src = Pointer(to=l)

    def whole(self) -> ref[Self.o._get_owned_interior["element"]] List[Int]:
        return self.src[]


def main():
    var l: List[Int] = [1, 2]
    var it = It(l)
    print(len(it.whole()))
