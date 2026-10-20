# A conformer's parameterized `comptime` value must declare the
# requirement's parameter types.
# expect: comptime member 'get' does not match the requirement
trait HasGet:
    comptime get[idx: Int]: Int


struct A(HasGet):
    comptime get[idx: UInt]: Int = 3


def main():
    print(A.get[1])
