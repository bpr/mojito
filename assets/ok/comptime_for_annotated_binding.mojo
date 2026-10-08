# A `comptime` binding with a converting annotation inside a `comptime for`
# over a method's or a `def`'s own binder (`comptime s: Float64 = 1.5`,
# `comptime t: String = "s"`) is served by the template: no call clones the
# body, and the binding converts as a runtime binding of the annotation does.


def scaled[n: Int]():
    comptime for i in range(n):
        comptime s: Float64 = 1.5
        comptime t: String = "d"
        comptime x: Float64 = Float64(i) * 2.5
        print(t, i, s, x)


struct Plain:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def show[n: Int](self):
        comptime for i in range(n):
            comptime s: Float64 = 0.5
            comptime t: String = "p"
            print(t, i, s, self.v)


struct Boxed[T: Writable & Copyable & Deinitable]:
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def show[n: Int](self):
        comptime for i in range(n):
            comptime t: String = "b"
            print(t, i, self.item)


def main():
    scaled[2]()
    Plain(7).show[2]()
    Plain(8).show[1]()
    Boxed[Int](3).show[2]()
    Boxed[String]("q").show[1]()
    comptime label: String = "top"
    print(label)
