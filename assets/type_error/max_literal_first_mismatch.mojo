# `max(2, UInt8(1))` materializes `2` to `Int` before it binds the `SIMD`
# overload's `x`, so `y: type_of(x)` rejects the `UInt8`, as at the pin.
# expect: no overload matches
def main():
    print(max(2, UInt8(1)))
