# expect: expected a compile-time Int index
# A variadic struct's dependent accessor takes its index as a compile-time
# parameter; a runtime value in that position is rejected, as the pinned Mojo
# rejects it ("cannot use a dynamic value in a parameter list").
struct Pair[*Ts: Copyable & Movable & Deinitable](Copyable, Movable):
    var storage: Tuple[*Self.Ts]

    def __init__(out self, var *args: *Self.Ts):
        self.storage = Tuple(*args^)

    def __getitem__[i: Int](self) -> Self.Ts[i]:
        return self.storage[i].copy()


def main():
    var p = Pair[Int, Bool](1, True)
    var i = 0
    print(p.__getitem__[i]())
