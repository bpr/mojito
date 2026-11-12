# A type parameter's default may apply a parametric type through a
# one-operand bracket (`G[3]`), which reads as the same application its
# multi-operand spelling does: the default is the type `G[3]`.

struct G[n: Int]:
    @staticmethod
    def get() -> Int:
        return Self.n


struct H[T: AnyType = G[3]]:
    @staticmethod
    def hello() -> Int:
        return 1


def main():
    print(H.hello())
    print(G[3].get())
