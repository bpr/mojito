# A struct may take a struct-instance value parameter: the argument
# expression evaluates through VM-backed CTFE, freezes, and keys the
# specialization — the same frozen layout at two sites is one type.
from layout import Layout

struct Tagged[l: Layout](Copyable, Movable):
    var scale: Int

    def __init__(out self, scale: Int):
        self.scale = scale

    def total(self) -> Int:
        var frozen = l
        return frozen.size() * self.scale

def main():
    var a = Tagged[Layout.row_major(2, 3)](10)
    print(a.total())
    var b: Tagged[Layout.row_major(2, 3)] = a
    print(b.total())
    var c = Tagged[Layout.col_major(4)](2)
    print(c.total())
