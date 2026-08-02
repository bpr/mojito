trait IteratorContract:
    comptime Element: Movable

    def __next__(mut self) -> Self.Element:
        ...

@fieldwise_init
struct Token(Movable):
    var value: Int

@fieldwise_init
struct TokenIterator(IteratorContract):
    comptime Element = Token

    var value: Token

    def __next__(mut self) -> ref[origin_of(self.value)] Token:
        return self.value

def main():
    pass
