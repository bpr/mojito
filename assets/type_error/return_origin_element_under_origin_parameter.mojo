# expect: cannot return reference with incompatible origin
# An element read through a pointer over an origin parameter is the list's
# element interior below that parameter, as the selected `List.__getitem__`'s
# result names it, so returning it under the parameter itself is rejected,
# as at the pin: 'origin_of(o["element"])' vs 'origin_of(o)'.
struct It[o: ImmOrigin]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] l: List[Int]):
        self.src = Pointer(to=l)

    def first(self) -> ref[Self.o] Int:
        return self.src[][0]


def main():
    var l: List[Int] = [1, 2]
    var it = It(l)
    print(it.first())
