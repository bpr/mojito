# expect: invalid call to 'len'
# `len` is upstream's overload set over `Sized` and `SizedRaising`: a struct
# defining `__len__` without declaring either conformance matches no
# overload (the pin: "no matching function in call to 'len'").
struct V:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __len__(self) -> Int:
        return self.n


def main():
    print(len(V(2)))
