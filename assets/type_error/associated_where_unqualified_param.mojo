# expect: unqualified access to struct parameter 'n'; use 'Self.n' instead
# An associated member's `where` clause reads its struct's parameter as
# `Self.n`, as a method's does.
struct W[n: Int]:
    comptime M: Int where n > 0 = 1

    def __init__(out self):
        pass


def main():
    print(W[2].M)
