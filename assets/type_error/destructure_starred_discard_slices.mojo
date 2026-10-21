# expect: invalid call to '__getitem__'
# A starred target stores the slice even when it is `*_`, so the value
# needs a slice `__getitem__`; this one indexes by `Int` only.
struct Trio(Sized, ImplicitlyCopyable):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def __len__(self) -> Int:
        return 3

    def __getitem__(self, i: Int) -> Int:
        return self.base + i


def main() raises:
    var t = Trio(10)
    var [a, *_, b] = t
    print(a, b)
