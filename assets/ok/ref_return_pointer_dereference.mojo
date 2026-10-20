# A returned pointer dereference names the pointer's own origin, as
# upstream's `Pointer.__getitem__(self) -> ref[Self.origin]` does: the
# origin-cast temporary of `Optional._unsafe_get_enum_payload`'s body
# returns under `ref[self]`, as do a cast held in a local and a pointer
# straight to a field under that field's origin.
@fieldwise_init
struct S(Copyable, Movable):
    var x: Int
    var y: Int

    def get(ref self) -> ref[self] Int:
        return Pointer(to=self.x).unsafe_origin_cast[origin_of(self)]()[]

    def get_local(ref self) -> ref[self] Int:
        var p = Pointer(to=self.y).unsafe_origin_cast[origin_of(self)]()
        return p[]

    def get_field(ref self) -> ref[self.y] Int:
        return Pointer(to=self.y)[]


def main():
    var s = S(5, 6)
    print(s.get(), s.get_local(), s.get_field())
    s.get() = 7
    s.get_local() += 1
    print(s.x, s.y)
    s.get_field() = 20
    print(s.y)
