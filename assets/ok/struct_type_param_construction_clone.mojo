# Constructing a generic struct's own type parameter (`Self.T()`) in a method
# and a constructor that each instance clones (each holds a `comptime if` on
# `Self.T`) constructs the bound type in every instance.

struct Box[T: Copyable & Deinitable & Defaultable & Writable]:
    var x: Self.T

    def __init__(out self):
        comptime if Self.T == Int:
            print("int box")
            self.x = Self.T()
        else:
            print("other box")
            self.x = Self.T()

    def __init__(out self, var x: Self.T):
        self.x = x^

    def reset(mut self):
        comptime if Self.T == Int:
            self.x = Self.T()
        else:
            self.x = Self.T()

    def fresh(self) -> Self.T:
        comptime if Self.T == Int:
            return Self.T()
        else:
            return Self.T()

    def get(self) -> Self.T:
        return self.x.copy()


def main():
    var f = Box[Float64](2.5)
    print(f.get(), f.fresh())
    f.reset()
    print(f.get())
    var i = Box[Int](7)
    print(i.get(), i.fresh())
    i.reset()
    print(i.get())
    var s = Box[String]("hi")
    print("[" + s.fresh() + "]")
    s.reset()
    print("[" + s.get() + "]")
    var d = Box[Float64]()
    print(d.get())
    var n = Box[Int]()
    print(n.get())
    var e = Box[String]()
    print("[" + e.get() + "]")
