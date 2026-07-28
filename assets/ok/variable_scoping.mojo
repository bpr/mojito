# `var` is required to declare a variable, and block scoping means a `var` inside
# an `if` does not leak to the enclosing block. Here the inner `var y = 4` shadows
# the outer `y` only within the branch, while `x = 4` reassigns the outer `x`.
def main():
    var x = 1
    var y = 1
    if True:
        x = 4
        print("inner x:", x)
        var y = 4
        print("inner y:", y)
    print("outer x:", x)
    print("outer y:", y)
