# expect: 'AnyStruct[List[Int]]' does not implement the '__and__' method
# A bound composition evaluates its `&` left to right, so a struct member
# is the left operand's missing `__and__`, not a trait.
def f[x: List[Int] & Copyable]():
    pass


def main():
    f[List[Int]()]()
