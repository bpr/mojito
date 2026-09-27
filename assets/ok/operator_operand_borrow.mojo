# A struct operator whose dunder reads its operands borrows named ones where
# they lie, and `hash(value)` hashes its argument in place: no lifecycle copy
# runs, so the printing copy constructor stays silent, as upstream.
from std.hashlib import Hasher


struct Box(Copyable, Equatable, Hashable):
    var items: List[Int]

    def __init__(out self, var items: List[Int]):
        self.items = items^

    def __init__(out self, *, copy: Self):
        print("copy")
        self.items = copy.items.copy()

    def __eq__(self, other: Self) -> Bool:
        return self.items == other.items

    def __lt__(self, other: Self) -> Bool:
        return len(self.items) < len(other.items)

    def __add__(self, other: Self) -> Self:
        var out = self.items.copy()
        for x in other.items:
            out.append(x)
        return Box(out^)

    def __hash__[H: Hasher](self, mut hasher: H):
        hasher._update_with_simd(Int64(len(self.items)))


def same[T: Equatable & Copyable](a: T, b: T) -> Bool:
    return a == b


def hash_both[T: Hashable](a: T, b: T) -> Bool:
    return Bool(hash(a) == hash(b))


def main():
    var b = Box([1, 2])
    var c = Box([1, 2])
    print(b == c, b != c, b == b, b < c)
    print(same(b, c))
    var d = b + c
    print(len(d.items))
    print(hash(b) == hash(c), hash_both(b, d))
