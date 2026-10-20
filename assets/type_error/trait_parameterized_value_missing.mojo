# A conformer must define a required parameterized `comptime` value.
# expect: missing comptime member 'get'
trait HasGet:
    comptime get[idx: Int]: Int


struct A(HasGet):
    pass


def main():
    pass
