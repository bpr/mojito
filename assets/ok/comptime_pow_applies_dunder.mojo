# A compile-time `**` over `Int` or `Float64` is the application of the
# declared `__pow__`, and a parameter's `**` folds as `_powi` computes it:
# a negative exponent gives 0 or ±1 rather than an error.
def f[n: Int]() -> Int:
    comptime k = n ** -1
    return k


def main():
    comptime a = Int(2) ** -1
    comptime b = 2.0 ** 3
    comptime c = 4.0 ** 0.5
    print(a, b, c, f[2](), f[-1]())
