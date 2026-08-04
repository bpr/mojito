# expect: expression must be mutable
# An unadorned `for` target is an immutable binding, independent of whether the
# source is borrowed or consumed. Mutating it is rejected; `for var item` or
# `for ref item` is required to mutate the loop item.
def main():
    var values = [1, 2]
    for item in values:
        item += 10
