# expect: ambiguous implicit conversion
# A literal both converting constructors accept, and neither takes at its
# default `Int`: no candidate is exact, so the conversion is ambiguous.
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
