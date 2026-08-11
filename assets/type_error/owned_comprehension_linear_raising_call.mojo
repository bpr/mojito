# expect: raising call in a comprehension would abandon
@explicit_destroy("close Conn")
struct Conn(Movable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        print("close", self.id)

def close_id(var c: Conn) raises -> Int:
    var id = c.id
    if id == 9:
        c^.close()
        raise Error("boom")
    c^.close()
    return id

def main():
    var conns: List[Conn] = [Conn(4), Conn(5)]
    try:
        var ids = [close_id(item^) for var item in conns^]
        print(len(ids))
    except:
        print("caught")
