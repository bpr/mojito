# expect: 'SIMDSize' was removed; use 'SIMDLength'
# Upstream removed the transitional `SIMDSize` width spelling (2026-08 window:
# `use of unknown declaration 'SIMDSize'`), and rejects the declaration that
# names it. Mojito no longer classifies it as a width value parameter either:
# the bound gets a targeted migration diagnostic where the declaration is
# classified, so the rejection lands on `lane_count` itself, as at the pin,
# rather than on the `[4]` argument its call supplies.
def lane_count[width: SIMDSize](v: SIMD[DType.int, width]) -> Int:
    return width

def main():
    print(lane_count[4](SIMD[DType.int, 4](0)))
