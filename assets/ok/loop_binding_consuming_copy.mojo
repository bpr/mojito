# Consuming a borrowed loop binding of an ImplicitlyCopyable element runs
# the referent's explicit copy initializer: the alias-bound element read must
# deep-copy an owning pointer field rather than alias it (previously a double
# free). A Copyable-only element needs `element.copy()` instead
# (assets/type_error/loop_binding_consuming_non_implicit.mojo).
from std.memory import unsafe_alloc

struct Buf(ImplicitlyCopyable, Movable, Writable):
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

    def write_to(self, mut writer: Some[Writer]):
        writer.write("buf")

def main() raises:
    var a = List[Buf]()
    a.append(Buf(1))
    var b = List[Buf]()
    for element in a:
        b.append(element)
    print(len(b), b[0], a[0])
