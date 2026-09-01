# The owned-`var` transfer convention's positive matrix: every owned position
# accepts `^` or `.copy()`, ImplicitlyCopyable values (String, Optional[Int],
# Tuple[Int, Bool], a struct declaring ImplicitlyCopyable with an explicit
# copy initializer, TrivialRegisterPassable-bounded parameters) copy
# implicitly, and operators bind their `var` operands the same way.
from std.optional import Optional

struct Pair(ImplicitlyCopyable):
    var left: Int
    var right: Int

    def __init__(out self, left: Int, right: Int):
        self.left = left
        self.right = right

    def __init__(out self, *, copy: Self):
        self.left = copy.left
        self.right = copy.right

def take(var values: List[Int]) -> Int:
    return len(values)

def dup[T: TrivialRegisterPassable](value: T) -> T:
    var copied: T = value
    return copied

def main():
    var a: List[Int] = [1, 2]
    print(take(a.copy()), len(a))
    var b = a.copy()
    b.append(3)
    print(len(a), len(b))
    var p: List[Int] = [1]
    var q: List[Int] = [2, 3]
    var joined = p + q.copy()
    print(len(joined), len(q))
    joined += q^
    print(len(joined))
    var s = String("text")
    var t = s
    t += "!"
    print(s, t)
    var maybe = Optional[Int](4)
    var again = maybe
    print(again.or_else(0), maybe.or_else(0))
    var tuple: Tuple[Int, Bool] = (7, True)
    var tuple_copy = tuple
    print(tuple_copy[0], tuple[1])
    var pair = Pair(1, 2)
    var pair_copy = pair
    pair_copy.left = 9
    print(pair.left, pair_copy.left)
    print(dup(41))
    print(take(a^))
