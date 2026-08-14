# expect: requires 'Movable & Deinitable' elements
from std.optional import Optional

@explicit_destroy("close Conn")
struct Conn(Movable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        print("close", self.id)

def main():
    var opt = Optional[Conn](init_with=lambda () -> Conn: Conn(1))
    for var item in opt^:
        item^.close()
