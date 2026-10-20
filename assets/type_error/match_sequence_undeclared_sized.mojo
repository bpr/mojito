# expect: value of type 'S' does not conform to 'Sized'
# The subject's declared conformance decides, not its methods: a struct
# with `__len__` and `__getitem__` that does not declare `Sized` is
# rejected.
struct S(Copyable):
    def __init__(out self):
        pass

    def __len__(self) -> Int:
        return 2

    def __getitem__(self, i: Int) -> Int:
        return i


def main():
    var s = S()
    match s:
        case [a, b]:
            print(a, b)
        case _:
            print("other")
