# A struct may take a struct-instance value parameter: the argument
# expression evaluates through VM-backed CTFE, freezes, and keys the
# specialization — the same frozen value at two sites is one type.
@fieldwise_init
struct Extent(Copyable, Movable):
    var rows: Int
    var cols: Int

    @staticmethod
    def square(n: Int) -> Extent:
        return Extent(n, n)

    def size(self) -> Int:
        return self.rows * self.cols

struct Tagged[e: Extent](Copyable, Movable):
    var scale: Int

    def __init__(out self, scale: Int):
        self.scale = scale

    def total(self) -> Int:
        var frozen = e
        return frozen.size() * self.scale

def main():
    var a = Tagged[Extent(2, 3)](10)
    print(a.total())
    var b: Tagged[Extent(2, 3)] = a.copy()
    print(b.total())
    var c = Tagged[Extent.square(4)](2)
    print(c.total())
