# An owning temporary passed to a read parameter, or read for a field, is
# destroyed once the consuming call returns, running its `__deinit__` as the
# pinned Mojo does. The checker marks a non-place argument at a read
# parameter (`ReadTemporaryArgument`) and lowering binds it to a hidden slot
# the call reads, as a named local is; a field read binds the temporary to a
# slot and loads the projected place, so the load keeps it alive through the
# consumer. A `var` parameter takes the temporary and destroys it itself.
struct B:
    var x: Int
    var name: String

    def __init__(out self, x: Int):
        self.x = x
        self.name = String("b") + String(x)

    def __deinit__(deinit self):
        print("deinit", self.x)

    def get(self) -> Int:
        return self.x

    def twin(self) -> B:
        return B(self.x * 10)


def take(b: B):
    print("take", b.x)


def own(var b: B):
    print("own", b.x)


def two(a: B, b: B):
    print("two", a.x, b.x)


def named(*, b: B):
    print("named", b.x)


def text(s: String):
    print("text", s)


def main():
    print("a read parameter")
    take(B(1))
    print("b owned parameter: the callee destroys it")
    own(B(2))
    print("c two temporaries")
    two(B(3), B(4))
    print("d keyword temporary")
    named(b=B(5))
    print("e scalar field fed to print")
    print(B(6).x)
    print("f scalar field bound")
    var y = B(7).x
    print("y", y)
    print("g field in arithmetic")
    print(B(8).x + 1)
    print("h owning field fed to print")
    print(B(9).name)
    print("i method result and field in one call")
    print(B(10).get(), B(11).x)
    print("j owning field stored")
    var n = B(12).name
    print("n", n)
    print("k owning field passed")
    text(B(13).name)
    print("l field of a chained temporary")
    print(B(14).twin().name)
    print("end")
