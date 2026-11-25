# PROBE: a reference into an origin parameter's owned element interior
# returned under the parameter itself.
#
# The pin rejects it: "cannot return reference with incompatible origin:
# 'origin_of(o["element"])' vs 'origin_of(o)'".
#
# Mojito accepts it. The return check compares a returned place against the
# declared origin, but a place reached through a pointer or `ref` field over
# `Self.o` is no place of the body, so the element interior below the
# parameter goes unseen. A caller then holds an element reference whose
# origin names the whole list, which a structural mutation does not stale.
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
