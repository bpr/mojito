# Members of the bundled `Tuple`, a struct specialized whole per element
# list, over two- and three-element instances of scalar, `String`, and
# user-struct elements: equality, ordering, hashing, `len`, and printing.
# Each instance's comparison, hash, and write read the pack's elements by
# loop index, one copy per element.

from std.hashlib import Hasher


@fieldwise_init
struct Tag(Equatable, Hashable, ImplicitlyCopyable, Writable):
    var rank: Int

    def __eq__(self, other: Self) -> Bool:
        return self.rank == other.rank

    def __hash__[H: Hasher](self, mut hasher: H):
        self.rank.__hash__(hasher)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Tag(", self.rank, ")")


def main():
    var pair = (String("b"), 1)
    var words = (String("x"), String("y"), String("z"))
    var tagged = (Tag(2), String("x"), 3)

    print(pair)
    print(words)
    print(tagged)
    print(len(pair), len(words), len(tagged))

    print(pair == (String("b"), 1), pair != (String("b"), 2))
    print(pair < (String("b"), 2), pair <= (String("a"), 9))
    print(pair > (String("a"), 9), pair >= (String("b"), 1))
    print(words < (String("x"), String("z"), String("a")))
    print(words > (String("x"), String("y"), String("y")))
    print(tagged == (Tag(2), String("x"), 3))
    print(tagged != (Tag(2), String("x"), 4))

    print(hash(pair) == hash((String("b"), 1)))
    print(hash(pair) == hash((String("b"), 2)))
    print(hash(tagged) == hash((Tag(2), String("x"), 3)))
