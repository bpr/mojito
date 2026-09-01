# expect: does not conform to trait 'Hashable'
# The retired `__hash__(self) -> UInt` shape is not the Hashable protocol: a
# conformer must contribute to a caller-owned hasher.
struct Token(Hashable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __hash__(self) -> UInt:
        return UInt(self.id)

def main():
    var token = Token(1)
    print(hash(token))
