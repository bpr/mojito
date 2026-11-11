# expect: invalid redefinition of 'M': cannot convert '__generator_type[k: Int] Int' to the other trait's member's type '__generator_type[j: Int] Int'
# A parameterized `comptime` requirement has a generator type, parameter
# names included, so a refinement that restates it must keep the names.
trait HasM:
    comptime M[k: Int]: Int


trait MoreM(HasM):
    comptime M[j: Int]: Int


struct S(MoreM):
    comptime M[i: Int]: Int = i * 3


def use[T: MoreM]() -> Int:
    return T.M[2]


def main():
    print(use[S]())
