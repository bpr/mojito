# A conformer's parameterized `comptime` value may not add a `where` clause
# the requirement does not state.
# expect: associated member constraint is not satisfied
trait HasGet:
    comptime get[idx: Int]: Int


struct A(HasGet):
    comptime get[idx: Int]: Int where idx > 0 = idx * 2


def use[T: HasGet]() -> Int:
    return T.get[0]


def main():
    print(use[A]())
