# A trait's parameterized `comptime` value requirement is a generator; an
# unparameterized member does not witness it.
# expect: comptime member 'get' does not match the requirement
trait HasGet:
    comptime get[idx: Int]: Int


struct A(HasGet):
    comptime get: Int = 3


def main():
    print(A.get)
