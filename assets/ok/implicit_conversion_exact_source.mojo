# Two converting constructors accept an `Int`; the one whose parameter is
# exactly the source type wins, and a literal converts at its default `Int`.
struct Number:
    var value: Int

    @implicit
    def __init__(out self, value: Int):
        self.value = value

    @implicit
    def __init__(out self, value: UInt):
        self.value = Int(value) + 100

def consume(value: Number):
    print(value.value)

def main():
    consume(1)
    var unsigned: UInt = 2
    consume(unsigned)
