@fieldwise_init
struct Grid:
    var value: Int

    def __getitem__(ref self, row: Int, column: Int) -> Int:
        print("multi get", row, column)
        return self.value

    def __setitem__(mut self, row: Int, column: Int, value: Int):
        print("multi set", row, column, value)
        self.value = value

@fieldwise_init
struct Window:
    var value: Int

    def __getitem__(ref self, span: Slice) -> Int:
        print("slice get")
        return self.value

    def __setitem__(mut self, span: Slice, value: Int):
        print("slice set", value)
        self.value = value

def first() -> Int:
    print("first")
    return 1

def second() -> Int:
    print("second")
    return 2

def amount() -> Int:
    print("rhs")
    return 3

def main():
    var grid = Grid(10)
    grid[first(), second()] += amount()
    print(grid.value)
    var window = Window(20)
    window[first():second()] += amount()
    print(window.value)
