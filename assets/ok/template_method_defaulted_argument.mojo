# A per-instantiation method clone inherits its checked template's facts when
# its body calls a method and leaves a defaulted parameter to its default
# (`docs/notes/instantiation-from-template.md`, class MethodBody). The default
# is the callee's declaration, evaluated in the callee's scope, so the call
# records only the omitted slot, alike under an `Int` and a `String` instance:
# on a `String` field, on a closed struct field, on `self`, and on the `^`
# transfer of a local to a named `deinit self` destructor.
struct Tag(Copyable, Movable):
    var text: String

    def __init__(out self, var text: String):
        self.text = text^

    def joined(self, sep: String = "!", times: Int = 1) -> String:
        return self.text + sep * times

    def reap(deinit self, bonus: Int = 1) -> Int:
        return self.text.byte_length() + bonus


struct Named[T: ImplicitlyCopyable & Deinitable & Writable](Movable):
    var item: Self.T
    var name: String
    var tag: Tag

    def __init__(out self, var item: Self.T, var name: String, var tag: Tag):
        self.item = item^
        self.name = name^
        self.tag = tag^

    def position(self) -> Int:
        return self.name.find("y")

    def starts(self) -> Bool:
        return self.name.startswith(" x")

    def tag_joined(self) -> String:
        return self.tag.joined()

    def tag_twice(self) -> String:
        return self.tag.joined(times=2)

    def reaped(self) -> Int:
        var tag = self.tag.copy()
        return tag^.reap()

    def scaled(self, value: Int, factor: Int = 3) -> Int:
        return value * factor

    def tripled_length(self) -> Int:
        return self.scaled(self.name.byte_length())


def main():
    var a = Named[Int](1, String("  abzz  "), Tag(String("a-b")))
    var b = Named[String](String("s"), String(" xyz"), Tag(String("cab")))
    print(a.position(), b.position(), a.starts(), b.starts())
    print(a.tag_joined(), b.tag_joined(), a.tag_twice(), b.tag_twice())
    print(a.tripled_length(), b.tripled_length(), a.reaped(), b.reaped())
