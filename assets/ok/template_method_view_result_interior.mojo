# A per-instantiation method clone inherits its checked template's facts when
# its body calls a method whose declared return origin projects an owned
# interior of its receiver (`docs/notes/instantiation-from-template.md`,
# class MethodBody, table `ViewResultInteriors`). `String.strip` returns a
# `StringSpan` over `origin_of(self)._get_owned_interior["bytes"]`: the tags
# and the immutable binder the call records are the callee's declaration,
# the same under every instance, so a `var` view of a field of `self` and
# one of a `var` local derive for an `Int` and a `String` instance.
struct Named[T: ImplicitlyCopyable & Deinitable & Writable](Movable):
    var item: Self.T
    var name: String

    def __init__(out self, var item: Self.T, var name: String):
        self.item = item^
        self.name = name^

    def trimmed_length(self) -> Int:
        var view = self.name.strip()
        return view.byte_length()

    def local_trimmed_length(self, var text: String) -> Int:
        var copy = text^
        var view = copy.lstrip()
        return view.byte_length()


def main():
    var a = Named[Int](1, String("  ab  "))
    var b = Named[String](String("s"), String(" xyz"))
    print(a.trimmed_length(), b.trimmed_length())
    print(a.local_trimmed_length(String("  cd")), b.local_trimmed_length(String("e")))
