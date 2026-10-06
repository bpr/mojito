# A parameter argument computed from a runtime value is no compile-time
# value: the function lifted for such an argument reads the generic body's
# parameters and local `comptime` bindings alone.

def g[e: Int]():
    print(e)

def h(x: Int) -> Int:
    return x * 2

def f[n: Int](x: Int):
    g[h(x + n)]()

def main():
    f[2](3)
