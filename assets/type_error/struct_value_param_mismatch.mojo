# expect: type mismatch for variable 'c'
# Different frozen struct values parameterize distinct specializations.
@fieldwise_init
struct Extent(Copyable, Movable):
    var rows: Int
    var cols: Int

struct Tagged[e: Extent](Copyable, Movable):
    var scale: Int

    def __init__(out self, scale: Int):
        self.scale = scale

def main():
    var a = Tagged[Extent(2, 3)](1)
    var c: Tagged[Extent(3, 2)] = a
    print(c.scale)
