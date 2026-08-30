# P1c: iterator declared before its container, reaching the container only
# through method calls on the ref field (the exact list.mojo shape).
from std.iterable import StopIteration


@fieldwise_init
struct BoxIter[o: Origin[mut=False]]:
    var src: ref[o] Box
    var index: Int

    def __next__(mut self) raises StopIteration -> Int:
        if self.index >= self.src.length():
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src.get(r)


struct Box:
    var items: List[Int]

    def __init__(out self):
        self.items = [7, 8, 9]

    def length(self) -> Int:
        return len(self.items)

    def get(self, i: Int) -> Int:
        return self.items[i]

    def __iter__(ref self) -> BoxIter[origin_of(self)]:
        ref source = self
        return BoxIter(source, 0)


def main():
    var b = Box()
    for x in b:
        print(x)
