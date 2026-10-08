# A body's compile-time application of a `def` generic over a type is a
# request the elaborator below MIR serves, whose instance reads the bound
# type's associated member (`T.size`), whether the body applies it
# directly or through a plain `def` that does.
trait HasSize:
    comptime size: Int

struct Buffer[n: Int](HasSize):
    comptime size = Self.n * 2

def capacity[T: HasSize]() -> Int:
    return T.size + 1

def outer() -> Int:
    return capacity[Buffer[8]]() + 100

def main():
    comptime c = capacity[Buffer[8]]()
    print(c)
    comptime d = outer()
    print(d)
    print(comptime(capacity[Buffer[2]]() * 2))
