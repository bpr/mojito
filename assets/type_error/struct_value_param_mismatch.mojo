# expect: type mismatch for variable 'c'
# Different frozen layout values parameterize distinct specializations.
from layout import Layout

struct Tagged[l: Layout](Copyable, Movable):
    var scale: Int

    def __init__(out self, scale: Int):
        self.scale = scale

def main():
    var a = Tagged[Layout.row_major(2, 3)](1)
    var c: Tagged[Layout.col_major(2, 3)] = a
    print(c.scale)
