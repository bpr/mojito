# expect: residual elements would require explicit destruction (close Conn)
@explicit_destroy("close Conn")
struct Conn(Movable, ImplicitlyDeletable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        print("close", self.id)

def drain(var conns: List[Conn]) -> Int:
    for var item in conns^:
        if item.id == 2:
            return item.id
        item^.close()
    return 0

def main():
    var conns = [Conn(1), Conn(2), Conn(3)]
    print(drain(conns^))
