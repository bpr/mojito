# A conformer's parameterized `comptime` value must produce the
# requirement's result type.
# expect: comptime member 'get' does not match the requirement
trait HasGet:
    comptime get[idx: Int]: Int


struct A(HasGet):
    comptime get[idx: Int]: Bool = True


def main():
    print(A.get[0])
