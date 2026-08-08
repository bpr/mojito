# Consuming a borrowed loop binding runs the referent's __copyinit__:
# the alias-bound element read must deep-copy an owning pointer field
# rather than alias it (previously a double free).
struct Buf(Copyable, Movable, Writable):
    var data: UnsafePointer[Byte]

    def __init__(out self, seed: Int):
        self.data = UnsafePointer[Byte].alloc(1)
        self.data[0] = 65

    def __init__(out self, *, copy: Self):
        self.data = UnsafePointer[Byte].alloc(1)
        self.data[0] = copy.data[0]

    def __init__(out self, *, deinit move: Self):
        self.data = move.data^

    def __del__(deinit self):
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
