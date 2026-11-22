# expect: aliasing values passed mutably to 'a' argument and passed mutably to 'b' argument in 'bump' call
# A method's `ref[o]` result over an origin binder only its bracket binds
# borrows that origin, so it aliases a `mut` argument naming the same place,
# as at the pin.
struct Holder[po: MutOrigin]:
    var p: Pointer[Int, Self.po]

    def __init__(out self, p: Pointer[Int, Self.po]):
        self.p = p

    def get[o: MutOrigin](self) -> ref[o] Int:
        return self.p.unsafe_origin_cast[o]()[]


def bump(mut a: Int, mut b: Int):
    a += 1
    b += 10


def main():
    var x = 1
    var y = 2
    var h = Holder(Pointer(to=x))
    bump(h.get[origin_of(y)](), y)
    print(x, y)
