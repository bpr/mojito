# expect: consuming receiver of method 'reverse'
# `Tuple.reverse` consumes its receiver: a place of non-copyable elements
# must be transferred with `^`.
@fieldwise_init
struct Token(Movable):
    var id: Int


def main():
    var pair = Tuple(Token(1), Token(2))
    var reversed = pair.reverse()
    print(reversed[0].id)
