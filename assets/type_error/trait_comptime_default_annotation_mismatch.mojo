# A trait `comptime` requirement's default is checked against its
# annotation at the trait, before any conformer takes it.
# expect: type mismatch for comptime member 'K': expected Int, found StringLiteral["x"]
trait T:
    comptime K: Int = "x"


struct S(T):
    pass


def main():
    print(S.K)
