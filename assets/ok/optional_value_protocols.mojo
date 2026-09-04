# Optional's value protocols (upstream `collections/optional.mojo`): the
# `@implicit` value constructor, `Writable` (payload text or `None`),
# conditional `Equatable`, conditional `Hashable`, the consuming
# `or_else(deinit self, var default)`, and the unchecked accessors.
@fieldwise_init
struct Res(Copyable, Movable, Writable):
    var id: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Res(", self.id, ")")

def pick(x: Optional[Int] = None) -> Int:
    return x.or_else(0)

def main() raises:
    var a: Optional[Int] = 5
    var empty = Optional[Int]()
    print(a)
    print(empty)
    print(Optional[String]("hi"))
    print(pick(7), pick(None))

    var b: Optional[Int] = 5
    var c: Optional[Int] = 6
    print(a == b, a != b, a == c, a == empty, empty == Optional[Int]())

    print(hash(Optional[Int](3)) == hash(Optional[Int](3)))
    print(hash(Optional[Int]()) == hash(Optional[Int]()))

    var held = Optional[Res](Res(1))
    var taken = held^.or_else(Res(9))
    print(taken)
    var missing = Optional[Res]()
    print(missing^.or_else(Res(9)))

    var d: Optional[Int] = 11
    print(d.unsafe_value())
    print(d.unsafe_take())
    print(Bool(d))
    var e: Optional[Int] = 4
    print(e.bounds()[0], Optional[Int]().bounds()[0])
