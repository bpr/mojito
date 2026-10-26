# A struct's value parameter reads as its declared type wherever `Self.x`
# names it: a `Bool`, a `Float64`, and a `String` parameter in an instance
# method, in a static method with no receiver, and as the receiver of a
# method call (`Self.s.byte_length()`). The pinned Mojo prints `ab! True`,
# `3.0 1.5 hi`, and `11.25 2`.
struct Tagged[s: String, f: Float64, b: Bool, u: UInt]:
    def __init__(out self):
        pass

    def label(self) -> String:
        return Self.s + "!"

    def flag(self) -> Bool:
        return Self.b and Self.u > 1

    def bytes(self) -> Int:
        return Self.s.byte_length()

    @staticmethod
    def name() -> String:
        return Self.s

    @staticmethod
    def scaled() -> Float64:
        return Self.f * 2


def main():
    var t = Tagged["ab", 0.5, True, 2]()
    print(t.label(), t.flag())
    print(
        Tagged["x", 1.5, False, 0].scaled(),
        Tagged["x", 1.5, False, 0].f,
        Tagged["hi", 0.0, False, 0].name(),
    )
    print(Tagged["cd", 5.625, False, 3].scaled(), t.bytes())
