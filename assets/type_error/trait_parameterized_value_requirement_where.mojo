# A parameterized `comptime` value requirement's `where` clause is checked
# at a read through a bound.
# expect: violated constraint
trait HasGet:
    comptime get[idx: Int]: Int where idx > 0


struct A(HasGet):
    comptime get[idx: Int]: Int where idx > 0 = idx * 2


def use[T: HasGet]() -> Int:
    return T.get[0]


def main():
    print(use[A]())
