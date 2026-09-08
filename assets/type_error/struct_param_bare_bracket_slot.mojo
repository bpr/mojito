# expect: unqualified access to struct parameter 'length'; use 'Self.length' instead
# A struct's own value parameter is spelled `Self.length` in a bracket slot
# inside its body, as upstream requires.
struct Counter[length: Int]:
    var i: Int

    def __init__(out self):
        self.i = 0

    def fresh(self) -> Int:
        var other = Counter[length]()
        return other.i + Self.length

def main():
    var c = Counter[4]()
    print(c.fresh())
