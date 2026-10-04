# Each iteration of a `comptime for` body is its own scope, in a generic
# `def` and in a method, plain or keyed: a `var` declared in the body is one
# binding per iteration. The pin prints `0` then `1` for each; Mojito used to
# report "'v' is already declared in this scope" for the generic `def`, whose
# clone spliced every unrolled copy into one block. The template now keeps the
# loop and the elaborator unrolls it below MIR.
struct Box[T: AnyType]:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def show(self):
        comptime for i in range(2):
            var v: Int = i + self.n
            print(v)

    def show_keyed[k: Int](self):
        comptime for i in range(k):
            var v: Int = i
            print(v)


def show[T: AnyType]():
    comptime for i in range(2):
        var v: Int = i
        print(v)


def main():
    show[Int]()
    Box[Int](10).show()
    Box[Int](0).show_keyed[2]()
