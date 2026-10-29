# A `ref` binding over a subscript whose `__getitem__` returns an owned
# value binds the materialized result, as a `ref` over any other owned call
# result does.
struct Names(Sized, Copyable):
    def __init__(out self):
        pass

    def __len__(self) -> Int:
        return 2

    def __getitem__(self, i: Int) -> String:
        return String("n") + String(i)


def main():
    var ns = Names()
    ref r = ns[0]
    print(r)
