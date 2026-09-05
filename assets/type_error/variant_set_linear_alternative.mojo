# expect: 'set' is unavailable for Variant[Conn]
from std.utils import Variant

@explicit_destroy("close Conn")
struct Conn(Movable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        print("close", self.id)

def main():
    var v: Variant[Conn] = Variant[Conn](Conn(3))
    v.set[Conn](Conn(4))
