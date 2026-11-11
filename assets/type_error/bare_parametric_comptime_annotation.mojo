# expect: cannot construct a value with parametric type: 'W[_]'
# A `comptime` binding annotated with a bare parametric struct names a type
# with unbound parameters, whose value cannot be constructed.
struct W[v: Int]:
    def __init__(out self):
        pass

def main():
    comptime x: W = W[1]()
    _ = x
