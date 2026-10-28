# `and`/`or` over a value that is not register-passable: the left operand is
# held where its `__bool__` reads it, and the selected operand moves into the
# result once, so the unselected one is destroyed and nothing else is copied.
# A place operand is copied into the result, which only an
# `ImplicitlyCopyable` value allows, so a merely `Copyable` one is spelled
# `.copy()`.
struct Tracked(Boolable, Copyable, Writable):
    var v: Int
    var name: String

    def __init__(out self, v: Int, name: String):
        self.v = v
        self.name = name

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        self.name = copy.name + "'"
        print("copy", self.name)

    def __deinit__(deinit self):
        print("del", self.name)

    def __bool__(self) -> Bool:
        return self.v != 0

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self.name)


def make(v: Int, name: String) -> Tracked:
    print("make", name)
    return Tracked(v, name)


def main():
    var r1 = make(0, "a") or make(1, "b")
    print("r1", r1)
    var r2 = make(2, "c") or make(3, "d")
    print("r2", r2)
    var r3 = make(0, "e") and make(4, "f")
    print("r3", r3)
    var x = Tracked(5, "x")
    var r4 = x.copy() or make(6, "g")
    print("r4", r4)
    var r5 = make(0, "h") or x^
    print("r5", r5)
    print("end")
