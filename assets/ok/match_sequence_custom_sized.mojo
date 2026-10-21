# Sequence patterns over a user `Sized` type with only `__getitem__(Int)`:
# one `__len__` per case gates the element reads, `_` reads nothing, and a
# from-end element after `*_` reads `__len__` again.
struct Trio(Sized, ImplicitlyCopyable):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def __len__(self) -> Int:
        print("len")
        return 3

    def __getitem__(self, i: Int) -> Int:
        print("get", i)
        return self.base + i


def main():
    var t = Trio(10)
    match t:
        case [a, b]:
            print("two", a, b)
        case [10, _, c]:
            print("three", c)
        case _:
            print("other")
    match t:
        case [_, *_, 12]:
            print("last 12")
        case _:
            print("other")
