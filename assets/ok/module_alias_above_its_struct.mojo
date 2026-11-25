# A module alias declared above the structs it names waits for them:
# a partially bound application and an unapplied struct are generators
# over the unbound parameters, a construction waits for the
# initializer, and a closed application is the type.
comptime First = Pair[_, 2]
comptime Q = Pair
comptime Made = Holder[Key(7)]
comptime Plain = Pair[3, 4]


@fieldwise_init
struct Key(ImplicitlyCopyable, Movable):
    var v: Int


struct Holder[k: Key](Defaultable):
    def __init__(out self):
        pass

    @staticmethod
    def get() -> Int:
        return Self.k.v


struct Pair[a: Int, b: Int](Defaultable):
    def __init__(out self):
        pass

    @staticmethod
    def sum() -> Int:
        return Self.a + Self.b


def main():
    print(First[1].sum(), Q[5, 6].sum(), Made.get(), Plain.sum())
