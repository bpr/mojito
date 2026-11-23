# An operator special method must be a method: a global function of
# that name is rejected at its declaration.
# expect: '__neg__' must be a method, not a global function
def __neg__(a: Int) -> Int:
    return -a


def main():
    print(__neg__(1))
