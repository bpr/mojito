# expect: type mismatch for argument to 'f': expected Box[1], found Box[2]
# A bound's parameterized value requirement in a signature closes at the
# conformer's member, so an argument of another value is rejected.


trait Tr:
    comptime get[idx: Int]: Int


struct S(Tr):
    comptime get[idx: Int]: Int = idx + 1


struct Box[n: Int]:
    def __init__(out self):
        pass


def f[T: Tr](x: Box[T.get[0]]):
    print(x.n)


def main():
    f[S](Box[2]())
