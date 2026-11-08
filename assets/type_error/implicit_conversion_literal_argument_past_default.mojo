# expect: expected Number, found IntLiteral[1]
# A literal reaches an `@implicit` constructor only at its own literal type
# or at the `Int` it materializes to: `Int8` and `Int16` would each be a
# second conversion, so neither constructor converts the argument.
struct Number:
    var value: Int

    @implicit
    def __init__(out self, value: Int8):
        self.value = Int(value)

    @implicit
    def __init__(out self, value: Int16):
        self.value = Int(value)

def consume(value: Number):
    pass

def main():
    consume(1)
