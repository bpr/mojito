# PROBE: a refining trait restates its parent's parameterized `comptime`
# requirement.
#
# Found while landing R1896 (2026-11-03) against `Mojo
# 1.3.0.dev2026110105`, which rejects the refinement: "invalid
# redefinition of 'M': cannot convert '__generator_type[k: Int] Int' to the
# other trait's member's type '__generator_type[j: Int] Int'". Mojito
# merges the two requirements and prints `6`.
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
