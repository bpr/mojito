# A per-instantiation method clone inherits its checked template's facts when
# its body passes a string literal to a method's parameter
# (`docs/notes/instantiation-from-template.md`, class MethodBody). The literal
# is a temporary of its own closed type, and its conversion to a `StringSpan`
# or a `String` parameter is fixed by its syntax and the callee's declared
# parameter, so a call on a `String` field, a `var` local, or a closed struct
# field derives for an `Int` and a `String` instance alike, and a view the
# call returns keeps its interior tags and immutable binder.
struct Tag(Copyable, Movable):
    var text: String

    def __init__(out self, var text: String):
        self.text = text^

    def count_in(self, chars: StringSpan) -> Int:
        return self.text.count(chars)

    def joined(self, sep: String) -> String:
        return self.text + sep

    def taken(self, var sep: String) -> Int:
        return sep.byte_length()


struct Named[T: ImplicitlyCopyable & Deinitable & Writable](Movable):
    var item: Self.T
    var name: String
    var tag: Tag

    def __init__(out self, var item: Self.T, var name: String, var tag: Tag):
        self.item = item^
        self.name = name^
        self.tag = tag^

    def trimmed_length(self) -> Int:
        var view = self.name.rstrip(" z")
        return view.byte_length()

    def position(self) -> Int:
        return self.name.find("y", 0)

    def has_z(self) -> Bool:
        return self.name.count("z") > 0

    def local_trimmed(self, var text: String) -> Int:
        var copy = text^
        var view = copy.lstrip("q")
        return view.byte_length()

    def tag_count(self) -> Int:
        return self.tag.count_in("a")

    def tag_joined(self) -> String:
        return self.tag.joined("!")

    def tag_taken(self) -> Int:
        return self.tag.taken("four")


def main():
    var a = Named[Int](1, String("  abzz  "), Tag(String("a-b-a")))
    var b = Named[String](String("s"), String(" xyz"), Tag(String("cab")))
    print(a.trimmed_length(), b.trimmed_length())
    print(a.position(), b.position(), a.has_z(), b.has_z())
    print(a.local_trimmed(String("qqab")), b.local_trimmed(String("c")))
    print(a.tag_count(), b.tag_count(), a.tag_joined(), b.tag_joined())
    print(a.tag_taken(), b.tag_taken())
