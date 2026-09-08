# expect: cannot access instance field 'index' without an instance of 'Counter[length]'
# `Self.index` names a field, not a parameter: a bracket slot is a
# compile-time position with no instance to read it from.
struct Counter[length: Int]:
    var index: Int

    def __init__(out self):
        self.index = 0

    def bad(self) -> Int:
        var other = Counter[Self.index]()
        return other.index

def main():
    var c = Counter[2]()
    print(c.bad())
