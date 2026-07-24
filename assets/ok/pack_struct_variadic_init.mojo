# Variadic-generic struct with real Mojo's pack constructor: `var *args: *Ts`
# binds the heterogeneous pack per-position and `Tuple(*args^)` transfers the
# elements into storage.
struct Pair[*Ts: Copyable & Movable](Copyable, Movable):
    var storage: Tuple[*Ts]

    def __init__(out self, var *args: *Ts):
        self.storage = Tuple(*args^)

    def size(self) -> Int:
        return len(self.storage)


def main():
    var p = Pair[Int, String, Bool](7, "x", False)
    print(p.size())
    print(p.storage[0])
    print(p.storage[1])
    print(p.storage[2])
