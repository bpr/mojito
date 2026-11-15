# An empty bracket binds none of a function's parameters, defaulted ones
# included, so the function read as a value stays parametric.
# expect: cannot use parametric function as a runtime closure: parameter 'b' of type 'Int' is not bound
def h[b: Int = 3](y: Int) -> Int:
    return y + b


def main():
    var g = h[]
    print(g(1))
