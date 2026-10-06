# `not (n > 9)` in a type is the negation of the comparison, a different
# value from the comparison `n <= 9` that decides the same.

def flag[b: Bool]():
    print(b)

struct Flag[b: Bool]:
    var x: Int

    def __init__(out self):
        self.x = 0

    def show(self):
        flag[Self.b]()

def gen[n: Int]() -> Flag[not (n > 9)]:
    return Flag[n <= 9]()

def main():
    gen[1]().show()
