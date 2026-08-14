# Keyword slice subscripts (`x[name=a:b]`): a named bracket argument whose
# value is a slice binds a keyword-only slice-descriptor `__getitem__`
# parameter through ordinary structural call binding. Omitted bounds are
# preserved, and an explicit second colon selects `StridedSlice`.
struct Grid:
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def __getitem__(self, *, byte: ContiguousSlice) -> Int:
        var start = byte.start.or_else(0)
        var end = byte.end.or_else(99)
        return self.base + start * 100 + end

    def __getitem__(self, *, stride: StridedSlice) -> Int:
        return self.base + stride.step.or_else(1) * 1000

def main():
    var g = Grid(7)
    print(g[byte=2:5])
    print(g[byte=:5])
    print(g[byte=2:])
    print(g[byte=:])
    print(g[stride=::3])
