# A reference returned through a pointer over an origin parameter names the
# origin the selected getter's result has: an element is the list's element
# interior below the parameter, and the dereferenced list the parameter
# itself.
struct It[o: ImmOrigin]:
    var src: Pointer[List[Int], Self.o]

    def __init__(out self, ref[Self.o] l: List[Int]):
        self.src = Pointer(to=l)

    def first(self) -> ref[Self.o._get_owned_interior["element"]] Int:
        return self.src[][0]

    def whole(self) -> ref[Self.o] List[Int]:
        return self.src[]


def main():
    var l: List[Int] = [1, 2]
    var it = It(l)
    print(it.first())
    print(len(it.whole()))
