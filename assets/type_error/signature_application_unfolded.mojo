# A non-builtin call in a signature is its application, never the value it
# computes: `Tag[1]` is not `Tag[idt(1)]`.
# expect: expected Tag[idt[Int](1)], found Tag[1]
struct Tag[k: Int]:
    def __init__(out self):
        pass


def idt[T: ImplicitlyCopyable](x: T) -> T:
    return x


def g[n: Int](t: Tag[idt(n)]):
    print(n)


def main():
    g[1](Tag[1]())
