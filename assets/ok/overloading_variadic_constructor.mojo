# A generic struct overloads an empty constructor against a variadic one. An
# explicit or inferred call with arguments selects the variadic overload and
# solves the element parameter from the overflow arguments; a zero-argument
# call prefers the fixed-arity empty constructor over the variadic one.

struct Bag[T: Copyable & Movable]:
    var count: Int

    def __init__(out self):
        self.count = -1

    def __init__(out self, var *values: Self.T):
        self.count = len(values)

def main():
    var explicit = Bag[Int](7, 8, 9)
    var empty = Bag[Int]()
    var inferred = Bag(1, 2, 3, 4)
    print(explicit.count)
    print(empty.count)
    print(inferred.count)
