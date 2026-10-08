# A list display with no contextual type is an `Array`, and as the temporary
# receiver of a `ref self` method (`[1, 2][0]`, `[1, 2].__getitem__(0)`) it
# is materialized in a hidden slot the statement destroys, as a call result
# is. The element is read, copied, or bound out of that slot, also in a
# generic body whose bound proves the element `Deinitable`.
struct Q(Copyable, Movable):
    var n: Int
    var s: String

    def __init__(out self, n: Int, s: String):
        self.n = n
        self.s = s


def first[T: Copyable & Writable & Deinitable](a: T, b: T) -> T:
    return [a.copy(), b.copy()][0].copy()


def main():
    print([1, 2][0])
    print(["a", "b"][1])
    print([[1, 2], [3, 4]][1][0])
    for i in range(2):
        print(["x", "y"][i])
    var s = ["p", "q"][1]
    s += "!"
    print(s)
    print(["a", "bc", "d"][1].byte_length())
    print([1, 2].__getitem__(0) + 1)
    print([Q(1, "x"), Q(2, "y")][0].s)
    print(first(3, 4))
    print(first(String("u"), String("v")))
    var x = [5, 6][1]
    print(x)
