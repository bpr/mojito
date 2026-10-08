# A `comptime for` over a sequence a compile-time application builds
# (`comptime for x in mk(n)`, or over a binding of one, `comptime l = mk(n)`
# then `comptime for x in l`) iterates the value the application returns per
# instance, from the template: no call clones the body.


def mk(n: Int) -> List[Int]:
    var values = List[Int]()
    for i in range(n):
        values.append(i * 10)
    return values^


def names(n: Int) -> List[String]:
    var values = List[String]()
    for i in range(n):
        values.append(String("n") + String(i))
    return values^


def each[n: Int]():
    comptime for x in mk(n):
        print("each", x)
    comptime for s in names(n):
        print("each", s)


struct Plain:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def each[n: Int](self):
        comptime l = mk(n)
        comptime for x in l:
            print("plain", x + self.v)


struct Boxed[T: Writable & Copyable & Deinitable]:
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def each[n: Int](self):
        comptime for s in names(n):
            print("boxed", s, self.item)


def fixed():
    comptime l = mk(2)
    comptime for x in l:
        print("fixed", x)


def main():
    each[2]()
    Plain(1).each[3]()
    Boxed[Int](4).each[2]()
    fixed()
