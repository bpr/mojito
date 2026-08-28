# A mutating method dispatched through a `ref`-typed field: the receiver is a
# stored reference handle, so the VM reads through it to dispatch and writes
# the mutation back through the same handle into the ultimate source.
@fieldwise_init
struct Drain[
    view_origin: Origin[mut=True],
]:
    var src: ref[view_origin] List[Int]

    def take(mut self) -> Int:
        return self.src.pop()

struct Box:
    comptime DrainType[view_origin: Origin[mut=True]] = Drain

    var items: List[Int]

    def __init__(out self):
        self.items = List[Int]()
        self.items.append(1)
        self.items.append(2)

    def drain(mut self) -> Self.DrainType[origin_of(self)]:
        ref source = self.items
        return Drain(source)

def main():
    var b = Box()
    var d = b.drain()
    print(d.take())
    print(d.take())
    print(len(b.items))
