# A generic struct's static that names the struct's parameter where no
# runtime argument or result carries it: an empty pack of `Self.T`, and a
# body-only use (`List[Self.T]()`). The spelled receiver alone selects the
# instance, from `main` and from another generic struct's method.


struct Pair[T: Copyable & Deinitable]:
    @staticmethod
    def count(*values: Self.T) -> Int:
        return len(values)

    @staticmethod
    def made() -> Int:
        var items = List[Self.T]()
        return len(items)


struct Outer[U: Copyable & Deinitable]:
    var x: Int

    def __init__(out self):
        self.x = 1

    def both(self) -> Int:
        return Pair[Self.U].count() + Pair[List[Self.U]].made() + self.x


def main():
    print(Pair[Int].count(), Pair[String].made(), Pair[List[Int]].made())
    print(Outer[String]().both())
