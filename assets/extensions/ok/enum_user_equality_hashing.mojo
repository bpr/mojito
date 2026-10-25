# An `enum` writes its own `__eq__` and `__hash__`, and keys a `Dict`, as
# the pin's integration test does; the default `Writable` text feeds both.
# Mojito spells the pin's `__enum` as `enum`, the spelling of upstream's
# enum proposal (`docs/non-goals.md`).
from std.hash import Hasher


enum Key(Equatable, Hashable, ImplicitlyCopyable, Writable):
    case named(String)
    case numbered(Int)
    case anonymous

    def __eq__(self, other: Self) -> Bool:
        return String(self) == String(other)

    def __hash__[H: Hasher](self, mut hasher: H):
        String(self).__hash__(hasher)


def main() raises:
    print(
        Key.named("a") == Key.named("a"),
        Key.named("a") == Key.numbered(1),
        Key.anonymous != Key.numbered(1),
    )
    var counts = Dict[Key, Int]()
    counts[Key.named("a")] = 1
    counts[Key.numbered(2)] = 2
    counts[Key.anonymous] = 3
    counts[Key.named("a")] += 10
    print(
        len(counts),
        counts[Key.named("a")],
        counts[Key.numbered(2)],
        counts[Key.anonymous],
    )
