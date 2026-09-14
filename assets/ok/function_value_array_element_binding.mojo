# An element of a thin function display binds to a local and is reassigned
# like any other value: a `def(...) thin` value captures nothing, so neither
# the binding nor the reassignment is a closure escape.
def double(x: Int) -> Int:
    return x * 2

def triple(x: Int) -> Int:
    return x * 3

def main():
    var fns = [double, triple]
    var f = fns[1]
    print(f(5))
    var g = fns[0]
    print(g(2))
    g = fns[1]
    print(g(2))
