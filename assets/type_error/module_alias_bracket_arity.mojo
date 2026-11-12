# expect: expects 2 type argument(s), got 3
# A module alias over a lowercase struct's bracket (`pair[1, 2, 3]`, a
# `MultiIndex`) is a parameter application, checked against the struct's
# parameters.


@fieldwise_init
struct pair[a: Int, b: Int](Copyable):
    pass


comptime P = pair[1, 2, 3]


def main():
    print(1)
