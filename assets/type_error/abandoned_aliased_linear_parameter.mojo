# An owned parameter spelled through a type alias of a linear struct is that
# struct: the verdict is asked of its type, not of the annotation's spelling.
# expect: 'x' abandoned without being explicitly destroyed: close me
@explicit_destroy("close me")
struct Lin(Deinitable where False, Movable):
    var x: Int

    def __init__(out self):
        self.x = 1

    def close(deinit self):
        pass


comptime L = Lin


def g(var x: L):
    pass


def main():
    pass
