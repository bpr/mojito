# `@__nonmaterializable(T)` decorates a struct only; on a function, as with
# no type argument, the pin knows no such decorator.
# expect: use of unknown declaration '__nonmaterializable'
@__nonmaterializable(Int)
def f():
    pass


def main():
    f()
