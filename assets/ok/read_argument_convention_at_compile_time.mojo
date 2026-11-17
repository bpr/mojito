# A concrete register-passable `read` argument is passed by value from IR
# generation on, under upstream's `imm` convention: `f(x: Int)` sees the 3
# it was passed even when the compile-time run executes it before any
# lowering (`cf` is 3). A template's `read x: T` is passed by address
# (`imm_mem`) until `lower-arg-conventions` loads it after elaboration, at
# every pipeline setting: at compile time `g` reads the write through its
# reference (`cg` is 5), at run time the loaded value (3).

@no_inline
def f(x: Int, p: Pointer[Int, MutUntrackedOrigin]) -> Int:
    p[] = 5
    return x

@no_inline
def g[T: ImplicitlyCopyable & Deinitable](
    x: T, p: Pointer[T, MutUntrackedOrigin], v: T
) -> T:
    p[] = v
    return x

def run_f() -> Int:
    var n = 3
    return f(n, Pointer(to=n).unsafe_origin_cast[MutUntrackedOrigin]())

def run_g() -> Int:
    var n = 3
    return g(n, Pointer(to=n).unsafe_origin_cast[MutUntrackedOrigin](), 5)

def main():
    comptime cf = run_f()
    comptime cg = run_g()
    print(cf, cg)
    print(run_f(), run_g())
