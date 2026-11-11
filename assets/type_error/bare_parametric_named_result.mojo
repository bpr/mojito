# expect: 'W[_]' is not concrete, use '[]' to bind missing parameters
# A named `out` result is the result, not an argument: its bare parametric
# struct is not concrete, as a `->` result's is.
struct W[v: Int]:
    def __init__(out self):
        pass

def g(out r: W):
    r = W[1]()

def main():
    _ = g()
