# expect: ambiguous implicit conversion
struct Number:
    var value: Int

    @implicit
    def __init__(out self, value: Int):
        self.value = value

    @implicit
    def __init__(out self, value: UInt):
        self.value = Int(value)

def consume(value: Number):
    pass

def main():
    consume(1)
