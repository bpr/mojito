# expect: cannot use a dynamic value in 'for' iterator expression
# A runtime variadic iterates, but it is not a compile-time value.
def f(*args: Int):
    comptime for a in args:
        print(a)


def main():
    f(1, 2)
