# A non-generic struct's generic constructor is served by its template: the
# elaborator instantiates `__init__[T]` per construction. A string literal
# argument binds `T` at `StringLiteral`, as the pin binds it, not at the
# `String` it would materialize to.
struct C:
    var text: String

    def __init__[T: Writable](out self, x: T):
        self.text = String(x)


def main():
    var a = C(3)
    var b = C("hi")
    var c = C(String("x"))
    var d = C(2.5)
    print(a.text, b.text, c.text, d.text)
