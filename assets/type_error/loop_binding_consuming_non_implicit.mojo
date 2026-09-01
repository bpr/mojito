# A borrowed loop binding of a Copyable-only element cannot be consumed
# implicitly: `append(var value)` needs an explicit `element.copy()`.
# expect: cannot be implicitly copied
from std.memory import unsafe_alloc

struct Buf(Copyable, Movable):
    var data: UnsafePointer[Byte]

    def __init__(out self, seed: Int):
        self.data = unsafe_alloc[Byte](1)
        self.data[0] = 65

    def __init__(out self, *, copy: Self):
        self.data = unsafe_alloc[Byte](1)
        self.data[0] = copy.data[0]

    def __init__(out self, *, deinit move: Self):
        self.data = move.data^

    def __deinit__(deinit self):
        self.data.free()

def main() raises:
    var a = List[Buf]()
    a.append(Buf(1))
    var b = List[Buf]()
    for element in a:
        b.append(element)
    print(len(b))
