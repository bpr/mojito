# `Tuple.reverse` and `Tuple.concat` as `std/builtin/tuple.mojo` declares
# them, with upstream's signatures: a named `out` result typed by a reversal
# and a concatenation of the element packs.


@fieldwise_init
struct Token(Movable):
    var id: Int


# A transform on a `Tuple` over a pack still open: the result type is the
# reversal of the def's own pack.
def flip[*Ts: Movable](var t: Tuple[*Ts]) -> Tuple[*Ts.reverse()]:
    return t^.reverse()


def main():
    # A place receiver of implicitly copyable elements is copied.
    var pair = Tuple(1, True)
    var suffix = Tuple(2.5)
    var reversed = pair.reverse()
    var joined = pair.concat(suffix)
    print(pair[0], pair[1], suffix[0])
    print(reversed[0], reversed[1])
    print(joined[0], joined[1], joined[2])
    # A tuple literal operand, an explicit pack, and the empty tuple.
    var literal = pair.concat((7, 2.5))
    print(literal[2], literal[3])
    var explicit = pair.concat[Bool](Tuple(False))
    print(explicit[2])
    print(len(Tuple().concat(Tuple())), len(Tuple().reverse()))
    var flipped: Tuple[Bool, Int] = pair.reverse()
    print(flipped[0])
    print((1, 2.5, False).reverse()[0])
    var chained = Tuple(5).reverse().concat(Tuple(6)).reverse()
    print(chained[0], chained[1])
    # Non-copyable elements move with `^`.
    var tokens = Tuple(Token(1), Token(2))
    var swapped = tokens^.reverse()
    print(swapped[0].id, swapped[1].id)
    var more = swapped^.concat(Tuple(Token(3)))
    print(len(more), more[0].id, more[2].id)
    var mixed = more^.concat((7, True))
    print(len(mixed), mixed[3], mixed[4])
    var generic = flip(Tuple(1, True, 2.5))
    print(generic[0], generic[1], generic[2])
