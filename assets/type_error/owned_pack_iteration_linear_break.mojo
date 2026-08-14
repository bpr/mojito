# Owned iteration over a variadic pack still forwards linear elements (there is
# no library iterator involved), so the escape guard remains the rejection for
# an abandoning exit.
# expect: residual elements would require explicit destruction (close Conn)
@explicit_destroy("close Conn")
struct Conn(Movable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        print("close", self.id)

def consume(var *conns: Conn):
    for var item in conns^:
        item^.close()
        break

def main():
    consume(Conn(1), Conn(2))
