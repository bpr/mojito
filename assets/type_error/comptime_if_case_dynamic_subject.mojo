# expect: cannot use a dynamic value in 'match' subject
# A `comptime if case` value is a compile-time value, the subject of the
# `comptime match` upstream checks the branch as; a runtime argument is the
# pin's rejection there.
def f(n: Int):
    comptime if case 3 = n:
        print("three")


def main():
    f(3)
