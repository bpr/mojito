# A memory-only truthiness operand is the receiver of its `__bool__`,
# which takes `read self`: a named value, a field, a generic `if a:`, a
# `not`, the left operand of `or`, and an `or` or conditional result tested
# again are each read where they lie, never copied into the call.
struct Flag(Boolable, Copyable):
    var on: Bool
    var name: String

    def __init__(out self, on: Bool):
        self.on = on
        self.name = "f"

    def __bool__(self) -> Bool:
        return self.on


struct H:
    var f: Flag

    def __init__(out self, var f: Flag):
        self.f = f^

    def check(self) -> Bool:
        if self.f:
            return True
        return False


def gbool[T: Boolable](a: T) -> Bool:
    if a:
        return True
    return False


def main():
    var f = Flag(True)
    if f:
        print("on")
    print(not f, gbool(f), gbool(3), gbool(String("")), H(f.copy()).check())
    var s = String("x")
    print(not s)
    print(s or String("y"))
    if Flag(False) or f.copy():
        print("or")
    var t = String("")
    if t if f else s:
        print("unreached")
    if s if f else t:
        print("ternary")
    while not t:
        t = "z"
    print(t)
