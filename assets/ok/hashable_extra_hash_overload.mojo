# A `__hash__` that takes no hasher is an ordinary overload: the struct still
# conforms to `Hashable` through the reflective field default.
struct Token(Hashable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __hash__(self) -> UInt:
        return UInt(self.id)

def main():
    print(hash(Token(1)) == hash(Token(1)))
    print(hash(Token(1)) == hash(Token(2)))
    print(Token(7).__hash__())
