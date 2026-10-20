# expect: cannot be indexed here
# A `Sized` subject without `__getitem__` cannot read its elements.
struct S(Sized, Copyable):
    def __init__(out self):
        pass

    def __len__(self) -> Int:
        return 1


def main():
    var s = S()
    match s:
        case [a]:
            print(a)
        case _:
            print("other")
