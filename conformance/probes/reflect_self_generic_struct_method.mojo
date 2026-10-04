# Pin gap probe (Mojo 1.2.0.dev2026092105): `reflect[Self]` in a method of a
# generic struct, written directly and inherited from a trait default that
# holds a `comptime if`. The pin prints `3` twice; Mojito reports "not a
# compile-time value: unsupported compile-time type argument". Roadmap R278
# (the plain-struct case is R269).
trait Describe:
    def kind(self) -> Int:
        comptime if reflect[Self].is_struct():
            return reflect[Self].field_count()
        else:
            return -1


@fieldwise_init
struct Box[T: Copyable & Deinitable](Describe):
    var a: Self.T
    var b: Self.T
    var c: Int

    def count(self) -> Int:
        return reflect[Self].field_count()


def main():
    var box = Box[Int](1, 2, 3)
    print(box.count())
    print(box.kind())
