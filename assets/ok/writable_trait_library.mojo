# `Writable` is the bundled `std/format` trait. A conformer without its own
# `write_to` or `write_repr_to` takes the reflective default, which names a
# generic instance by its parameters and writes each field through the
# field's own member; a declared trait may refine `Writable`, and a field
# that is not `Writable` matters only where the default is instantiated.


@fieldwise_init
struct Pair(Writable):
    var a: Int
    var b: String


@fieldwise_init
struct Empty(Writable):
    pass


@fieldwise_init
struct Holder[T: Copyable & Writable & Deinitable](Writable):
    var v: Self.T


@fieldwise_init
struct Shown[T: AnyType](Writable):
    var x: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Shown", self.x)


@fieldwise_init
struct Maybe[T: Copyable & Deinitable](Writable where conforms_to(T, Writable)):
    var v: Self.T


trait Tagged(Writable):
    def tag(self) -> Int:
        ...


@fieldwise_init
struct Tag(Tagged):
    var x: Int

    def tag(self) -> Int:
        return self.x


def show[T: Tagged](x: T):
    print(x, x.tag())


struct Opaque(Copyable):
    var p: Int

    def __init__(out self):
        self.p = 1


@fieldwise_init
struct Unprinted(Writable):
    var o: Opaque


def write_all[*Ts: Writable](mut w: String, *a: *Ts):
    comptime for i in range(a.__len__()):
        a[i].write_to(w)


def main():
    print(Pair(1, "s"))
    print(repr(Pair(1, "s")))
    print(Empty(), repr(Empty()))
    print(Holder[Int](5))
    print(repr(Holder[String]("w")))
    print(repr(Shown[Int](1)))
    print(Maybe[Int](7), repr(Maybe[Bool](True)))
    var s = String()
    Pair(2, "t").write_to(s)
    s.write(" ")
    Pair(3, "u").write_repr_to(s)
    print(s)
    show(Tag(4))
    var u = Unprinted(Opaque())
    print(u.o.p)
    var w = String()
    write_all(w, Shown[Int](1), 2, Pair(3, "v"))
    print(w)
    print("{} {!r}".format(Shown[Int](8), Pair(9, "x")))
    print(repr(Codepoint(65)))
