# expect: type mismatch for value parameter 'e': expected Int, found Float64
# A parameter argument that computes a `Float64` from a generic body's
# parameters binds only a `Float64` value parameter: the application is
# typed by its function's result.

def half(n: Int) -> Float64:
    return Float64(n) / 2.0

def g[e: Int]():
    print(e)

def f[n: Int]():
    g[half(n)]()

def main():
    f[3]()
