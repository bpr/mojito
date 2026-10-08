# A body's compile-time display, tuple, or subscript whose operands apply a
# callable is a request the elaborator below MIR serves, as is a method of
# a value binding (`d.get("a")`, `S.byte_length()`), which the request
# constructs itself.
def f(x: Int) -> Int:
    return x * 2

def k[n: Int]() -> Int:
    return n * 10

comptime L = [10, 20, 30]
comptime S = "abc"

def main():
    comptime M = [f(1), 2]
    var m = materialize[M]()
    print(m[0], m[1])
    comptime t = (f(3), 4)
    print(t[0], t[1])
    comptime u = (k[3](), 4)
    print(u[0])
    comptime v = L[f(1) - 1]
    print(v)
    comptime d = {"a": 1, "b": 5}
    comptime w = d.get("b").value()
    print(w)
    print(comptime(d.get("zz", 7)))
    comptime n = S.byte_length() + L.__len__()
    print(n)
