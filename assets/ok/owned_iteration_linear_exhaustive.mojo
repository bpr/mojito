@explicit_destroy("close Conn")
struct Conn(Movable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        print("close", self.id)

def main():
    var conns = [Conn(1), Conn(2), Conn(3)]
    for var item in conns^:
        item^.close()
    print("done")
