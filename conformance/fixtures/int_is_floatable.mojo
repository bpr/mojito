# Neither `Int` nor an integer literal conforms to `Floatable`, as upstream's
# `SIMD` and `IntLiteral` do not declare it, so a `Floatable`-bounded helper
# rejects one.
def to_flt[T: Floatable](x: T) -> Float64:
    return Float64(x)

def main():
    print(to_flt(4))
