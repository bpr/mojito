# An imported `comptime` alias of a struct application (`default_hasher`)
# types a parameter of a generic struct's method whose body branches at
# compile time, so each instance of the method resolves the alias. The
# pinned Mojo prints `True`.
from std.hash import default_hasher


struct Box[T: Copyable & Deinitable & Hashable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def feed(self, mut hasher: default_hasher):
        comptime if Self.T == Int:
            self.item.__hash__(hasher)
        else:
            self.item.__hash__(hasher)


def main():
    var hasher = default_hasher()
    Box[Int](3).feed(hasher)
    Box[String]("three").feed(hasher)
    print(hasher^.finish() > 0)
