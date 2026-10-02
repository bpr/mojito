# A `mut` parameter handed to a consuming callee is empty at the implicit
# return.
# expect: 'x' is uninitialized at return from this function
# requires: stdlib
def sink(var s: String):
    print(s)


def give(mut x: String):
    sink(x^)


def main():
    var s = String("a")
    give(s)
