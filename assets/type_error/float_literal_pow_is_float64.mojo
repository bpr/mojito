# A float literal's `**` is `Float64`'s `__pow__`, so its result is a
# `Float64`, not a literal that converts to `Float32`.
# expect: expected Float32, found Float64
def main():
    comptime b = 2.0 ** 3
    var y: Float32 = b
    print(y)
