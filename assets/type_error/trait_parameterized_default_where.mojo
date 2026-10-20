# A defaulted parameterized `comptime` requirement keeps its `where` clause
# at the conformer that takes it.
# expect: violated constraint
trait HasGet:
    comptime get[idx: Int]: Int where idx >= 0 = idx * 2


@fieldwise_init
struct A(HasGet):
    pass


def main():
    print(A.get[-1])
