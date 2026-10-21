# expect: invalid call to '__getitem__'
# A named star matches the subject's own slice, so the subject needs a
# slice `__getitem__`; this one indexes by `Int` only.
struct Trio(Sized, ImplicitlyCopyable):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def __len__(self) -> Int:
        return 3

    def __getitem__(self, i: Int) -> Int:
        return self.base + i


def main():
    var t = Trio(10)
    match t:
        case [a, *r]:
            print(a)
        case _:
            pass
