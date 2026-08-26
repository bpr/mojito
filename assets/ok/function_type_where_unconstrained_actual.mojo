# Passing an UNCONSTRAINED function where a constrained function type is
# expected is allowed and free (the directional half of upstream's 2026-08
# constrained-binding rule): the contract's `where` clause only restricts
# calls through the contract.
def kernel[w: Int](x: Int):
    print(w + x)

def apply[F: def[w: Int](Int) thin -> None where (w > 0, "width must be positive")](x: Int):
    F[4](x)

def main():
    apply[kernel](3)
