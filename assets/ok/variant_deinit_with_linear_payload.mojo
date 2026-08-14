# `deinit_with` is the linear-capable teardown: a non-Deinitable payload is
# handed to the consuming handler, so no implicit destructor is required.
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
    v.deinit_with(lambda (deinit element: Conn): element^.close())
    print("done")
