# expect: does not conform to trait 'ImplicitlyCopyable'
# A `for var` target over a reference-yielding iterator copies the referent into
# owned storage, so the element must be `ImplicitlyCopyable`. `Token` is only
# movable, so the copying `var` binding is rejected; `for item`/`for ref item`
# (which borrow the referent) remain available.
from std.iterable import StopIteration


struct Token(ImplicitlyDeletable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value


@fieldwise_init
struct TokenIter[o: Origin[mut=False]]:
    var src: ref[o] Token
    var done: Bool

    def __next__(mut self) raises StopIteration -> ref[o] Token:
        if self.done:
            raise StopIteration()
        self.done = True
        return self.src


struct Tokens:
    var value: Token

    def __init__(out self, value: Token):
        self.value = value

    def __iter__(ref self) -> TokenIter:
        ref v = self.value
        return TokenIter(v, False)


def main():
    var source = Tokens(Token(1))
    for var item in source:
        print(item.value)
