# expect: type parameter 'width' needs a type argument
# Upstream removed the transitional `SIMDSize` width spelling (2026-08 window:
# `use of unknown declaration 'SIMDSize'`). Mojito no longer classifies it as
# a width value parameter, so `SIMDSize` is just an unknown bound and the
# explicit `[4]` below is a value supplied to a type parameter. (Like every
# unknown bound, an uncalled declaration stays lazily unvalidated.)
def lane_count[width: SIMDSize](v: SIMD[DType.int, width]) -> Int:
    return width

def main():
    print(lane_count[4](SIMD[DType.int, 4](0)))
