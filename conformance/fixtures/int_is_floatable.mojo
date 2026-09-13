# Mojito conforms `Int` (and an integer literal) to `Floatable`, so a
# `Floatable`-bounded helper takes one; upstream conforms neither.
def to_flt[T: Floatable](x: T) -> Float64:
    return Float64(x)

def main():
    print(to_flt(4))
