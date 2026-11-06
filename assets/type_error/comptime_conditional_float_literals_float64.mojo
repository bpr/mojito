# expect: expected Float32, found Float64
# A compile-time conditional merges its arms as a run-time one does:
# `0.5` and `0.25` merge at `Float64`, which does not convert to `Float32`.
def main():
    comptime B = 0.5 if True else 0.25
    var x: Float32 = B
    print(x)
