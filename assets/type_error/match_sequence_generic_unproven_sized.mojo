# expect: cannot prove that value of type 'T' conforms to 'Sized'
# A generic subject's bounds decide: `G` requires only `__getitem__`, so
# nothing proves `T` is `Sized`.
trait G:
    def __getitem__(self, i: Int) -> Int:
        ...


def first[T: G](x: T) -> Int:
    match x:
        case [a]:
            return a
        case _:
            return -1


struct One(G, Copyable):
    def __init__(out self):
        pass

    def __getitem__(self, i: Int) -> Int:
        return i


def main():
    print(first(One()))
