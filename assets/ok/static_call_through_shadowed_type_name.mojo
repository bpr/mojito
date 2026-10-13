# A static call through a type name calls the type's static even after a
# block's local of the same spelling ends, and a live local of that spelling
# is still a value receiver inside its block.


@fieldwise_init
struct Counter(Copyable):
    var n: Int

    def get(self) -> Int:
        return self.n


struct Point:
    @staticmethod
    def origin() -> Int:
        return 7

    @staticmethod
    def shifted(by: Int) -> Int:
        return 7 + by


def main():
    for i in range(1):
        var Point = Counter(3)
        print(Point.get() + i)
    print(Point.origin())
    var p = Counter(4)
    print(p.get())
    print(Point.shifted(p.get()))
