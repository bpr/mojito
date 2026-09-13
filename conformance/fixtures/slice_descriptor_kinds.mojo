# Mojito splits upstream's single `Slice` descriptor into `ContiguousSlice`
# (two bounds) and `StridedSlice` (three), and a slice literal's second colon
# picks between them, so a subscript can overload on the kind. The pin has
# only `Slice` and rejects both names.
struct Grid:
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def __getitem__(self, part: ContiguousSlice) -> Int:
        return self.base + part.start.or_else(0)

    def __getitem__(self, part: StridedSlice) -> Int:
        return self.base + part.step.or_else(1) * 1000

def main():
    var g = Grid(7)
    print(g[2:5], g[2:5:3])
