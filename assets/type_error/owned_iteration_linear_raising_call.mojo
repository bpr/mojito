# expect: cannot contain an unhandled raising call
@explicit_destroy("close Conn")
struct Conn(Movable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self) raises:
        if self.id == 9:
            raise Error("boom")
        print("close", self.id)

def main():
    var conns: List[Conn] = [Conn(1), Conn(2)]
    try:
        for var item in conns^:
            item^.close()
    except:
        print("caught")
