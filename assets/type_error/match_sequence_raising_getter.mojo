# expect: call to raising method '__getitem__'
# An element read calls the subject's getter, so a raising getter needs a
# raising context.
struct S(Sized, Copyable):
    def __init__(out self):
        pass

    def __len__(self) -> Int:
        return 1

    def __getitem__(self, i: Int) raises -> Int:
        raise Error("boom")


def main():
    var s = S()
    match s:
        case [a]:
            print(a)
        case _:
            print("other")
