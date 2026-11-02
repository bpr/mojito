# A compile-time `**` over `Int` or `Float64` is the application of the
# declared `__pow__`, and a parameter's `**` folds as `_powi` computes it.
def f[n: Int]() -> Int:
    comptime k = n ** 3
    return k


def main():
    comptime a = Int(2) ** 10
    comptime b = 2.0 ** 3
    comptime c = 4.0 ** 0.5
    print(a, b, c, f[2](), f[-1]())
