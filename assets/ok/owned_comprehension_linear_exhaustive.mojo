@explicit_destroy("close Conn")
struct Conn(Movable, ImplicitlyDeletable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        print("close", self.id)

def close_id(var c: Conn) -> Int:
    var id = c.id
    c^.close()
    return id

def main():
    var conns = [Conn(4), Conn(5)]
    var ids = [close_id(item^) for var item in conns^]
    print(ids[0], ids[1])
    print("done")
