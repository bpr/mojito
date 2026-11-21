# A construction's argument must convert to the parameter type the supplied
# origin fixes: a pointer to the receiver's field carries the field's origin,
# not its owner's, as at the pin.
# expect: whose origin is the supplied 'o' argument
struct S[mut: Bool, //, o: Origin[mut=mut]]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, p: Pointer[Int, Self.o]):
        self.p = p


struct Holder:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def make(ref self) -> Int:
        var s = S[origin_of(self)](Pointer(to=self.x))
        return s.p[]


def main():
    var h = Holder(1)
    print(h.make())
