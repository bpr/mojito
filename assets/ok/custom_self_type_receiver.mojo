# `@__allow_legacy_custom_self_type` lets a method's `self` take any
# application of its struct: argument 0 at that type. The method's own
# binders are deduced from the receiver (`any_u`), `Self.T` in the
# annotation is the receiver's argument (`tname`), conventions combine with
# the custom type (`bump`, `take`), overloads may differ by receiver type
# alone (`k`), and the type-receiver, dereferenced, and compile-time call
# forms reach it too. On a plain `self` or `self: Self` it changes nothing.
@fieldwise_init
struct Box[T: Copyable]:
    var x: Int

    @__allow_legacy_custom_self_type
    def only_int(self: Box[Int]) -> Int:
        return self.x + 1

    @__allow_legacy_custom_self_type
    def any_u[U: Copyable, //](self: Box[U]) -> Int:
        return self.x + 2

    @__allow_legacy_custom_self_type
    def tname[U: Copyable, //](self: Box[U], u: Self.T) -> Int:
        return self.x

    @__allow_legacy_custom_self_type
    def bump(mut self: Box[Int]):
        self.x += 1

    @__allow_legacy_custom_self_type
    def take(var self: Box[Int]) -> Int:
        return self.x

    @__allow_legacy_custom_self_type
    def k(self: Box[Int]) -> Int:
        return 1

    @__allow_legacy_custom_self_type
    def k(self: Box[String]) -> Int:
        return 2

    @__allow_legacy_custom_self_type
    def plain(self) -> Int:
        return 3

    @__allow_legacy_custom_self_type
    def spelled(self: Self) -> Int:
        return 4


def main():
    var b = Box[Int](3)
    print(b.only_int(), b.any_u(), Box[String](4).any_u())
    print(Box[Int](1).tname(3))
    b.bump()
    print(b.x)
    print(Box[Int](0).k(), Box[String](0).k())
    print(Box[Int].only_int(Box[Int](5)))
    var r = Pointer(to=b)
    print(r[].only_int())
    comptime c = Box[Int](7).only_int()
    print(c)
    print(b.plain(), b.spelled())
    print(b^.take())
