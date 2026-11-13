# A user struct's `__getitem__` and method over an anonymous `Some[Indexer]`
# index: each call binds the parameter to its argument's type, so two index
# types are two instances, with or without the pre-elaboration pipeline.


struct Grid:
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def __getitem__(self, i: Some[Indexer]) -> Int:
        return self.base + index(i)

    def at(self, i: Some[Indexer]) -> Int:
        return self.base * index(i)


def main():
    var grid = Grid(10)
    print(grid[Int32(1)], grid[UInt8(3)], grid[2])
    print(grid.at(Int16(2)), grid.at(UInt(4)))
    var values: List[Int] = [10, 20, 30]
    print(values[Int32(1)], values[UInt8(2)])
