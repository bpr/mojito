# expect: doesn't have a memory origin in 'ref' binding
# An owned element read is an rvalue: `as` cannot name a register-passable
# one.
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
        case [10 as y, _, _]:
            print(y)
        case _:
            print("other")
