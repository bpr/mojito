# expect: invalid redefinition of 'T': cannot convert 'AnyType' to the other trait's member's type '__generator_type[k: Int] AnyType'
# Only an unparameterized associated type refines by accumulating bounds;
# giving it parameters in a refinement changes its type.
trait HasT:
    comptime T: AnyType


trait MoreT(HasT):
    comptime T[k: Int]: AnyType


def main():
    print(1)
