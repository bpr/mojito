# expect: non-Deinitable 'Conn' cannot be consumed implicitly
@fieldwise_init
struct StopIteration:
    pass

@explicit_destroy("close Conn")
struct Conn(Movable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        print("close", self.id)

struct Drain(Iterator, Deinitable where False, Movable):
    comptime Element = Conn
    var remaining: Int

    def __init__(out self, remaining: Int):
        self.remaining = remaining

    def __next__(mut self) raises StopIteration -> Conn:
        if self.remaining == 0:
            raise StopIteration()
        self.remaining -= 1
        return Conn(self.remaining)

struct Bucket(Movable):
    var count: Int

    def __init__(out self, count: Int):
        self.count = count

    def __iter__(var self) -> Drain:
        return Drain(self.count)

def main():
    var bucket = Bucket(2)
    for var item in bucket^:
        item^.close()
