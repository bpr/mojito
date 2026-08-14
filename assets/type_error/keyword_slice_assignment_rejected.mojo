# expect: keyword subscripts are read-only
# Keyword slice assignment is rejected like every keyword subscript
# assignment.
struct Grid:
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def __getitem__(self, *, byte: ContiguousSlice) -> Int:
        return self.base

def main():
    var g = Grid(7)
    g[byte=1:3] = 9
