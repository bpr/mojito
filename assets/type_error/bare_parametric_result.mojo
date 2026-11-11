# expect: 'W[_]' is not concrete, use '[]' to bind missing parameters
# A result must name a concrete type: a bare parametric struct leaves its
# parameters unbound, which only an argument's annotation infers.
struct W[v: Int]:
    def __init__(out self):
        pass

def g() -> W:
    return W[1]()

def main():
    _ = g()
