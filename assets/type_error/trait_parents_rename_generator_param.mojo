# expect: invalid redefinition of 'M': cannot convert '__generator_type[k: Int] Int' to the other trait's member's type '__generator_type[j: Int] Int'
# Two parents' parameterized requirements of one member merge only when
# their generator types agree, parameter names included.
trait A:
    comptime M[k: Int]: Int


trait B:
    comptime M[j: Int]: Int


trait C(A, B):
    pass


struct S(C):
    comptime M[i: Int]: Int = i * 3


def main():
    print(S.M[2])
