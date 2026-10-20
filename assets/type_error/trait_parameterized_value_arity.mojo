# A conformer's parameterized `comptime` value must take the requirement's
# parameters, no more.
# expect: comptime member 'get' does not match the requirement
trait HasGet:
    comptime get[idx: Int]: Int


struct A(HasGet):
    comptime get[idx: Int, j: Int]: Int = idx + j


def main():
    print(A.get[0, 1])
